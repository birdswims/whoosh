//! TLS for AirDrop.
//!
//! Receivers use a fresh self-signed certificate. Senders check that the peer
//! holds the private key for the certificate it presented, and do not require
//! a public web CA: AirDrop certificates chain to an Apple CA that is not
//! published for third-party clients.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::error::{Error, Result};
use crate::native::install_crypto;

pub fn server_acceptor() -> Result<(TlsAcceptor, CertificateDer<'static>)> {
    install_crypto();
    let (cert_der, key_der) = identity()?;
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der)
        .map_err(|error| Error::crypto(error.to_string()))?;
    Ok((TlsAcceptor::from(Arc::new(config)), cert_der))
}

fn identity() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    // AirDrop clients still offer only RSA cipher suites. An ECDSA certificate
    // makes the handshake fail with alert 40, and the iPhone then shows no row.
    // rcgen's default expiry is year 4096, which does not fit in the 32-bit
    // time some of those clients use.
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_RSA_SHA256)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let mut params = rcgen::CertificateParams::new(vec!["AirDrop".into()])
        .map_err(|error| Error::crypto(error.to_string()))?;
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2035, 1, 1);
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "AirDrop");
    let cert = params
        .self_signed(&key_pair)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let cert_der = CertificateDer::from(cert);
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    Ok((cert_der, key_der))
}

pub fn client_connector(expected: CertificateDer<'static>) -> Result<TlsConnector> {
    install_crypto();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(expected)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

pub fn interoperable_connector() -> Result<TlsConnector> {
    install_crypto();
    // AirDrop requests a client identity as well as presenting its own.
    // The legacy Everyone-mode protocol permits a self-signed identity.
    // This does not implement newer identity/code pairing or access Apple
    // account keys or validation records.
    let (certificate, key) = identity()?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SignatureOnly::new())
        .with_client_auth_cert(vec![certificate], key)
        .map_err(|error| Error::crypto(error.to_string()))?;
    Ok(TlsConnector::from(Arc::new(config)))
}

pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("AirDrop").expect("static dns name")
}

#[derive(Debug)]
struct SignatureOnly(Arc<rustls::crypto::CryptoProvider>);

impl SignatureOnly {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl ServerCertVerifier for SignatureOnly {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::server_acceptor;

    #[tokio::test]
    async fn interoperable_sender_supplies_client_identity() {
        use std::sync::Arc;
        let connector = super::interoperable_connector().unwrap();
        let identity = connector
            .config()
            .client_auth_cert_resolver
            .resolve(&[], &[rustls::SignatureScheme::RSA_PSS_SHA256])
            .expect("AirDrop sender must present a client certificate");
        let mut roots = rustls::RootCertStore::empty();
        roots.add(identity.cert[0].clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .unwrap();
        let (cert, key) = super::identity().unwrap();
        let config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (sent, received) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(
                connector.connect(super::server_name(), client),
                acceptor.accept(server)
            )
        })
        .await
        .unwrap();
        sent.unwrap();
        let received = received.unwrap();
        assert_eq!(
            received.get_ref().1.peer_certificates().unwrap(),
            identity.cert
        );
    }

    #[test]
    fn certificate_is_rsa_and_expires_before_2038() {
        let (_acceptor, cert) = server_acceptor().expect("acceptor");
        let der = cert.as_ref();
        const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
        assert!(
            der.windows(RSA_ENCRYPTION.len())
                .any(|window| window == RSA_ENCRYPTION),
            "AirDrop certificate is not RSA"
        );
        assert!(der.windows(13).any(|window| window == b"240101000000Z"));
        assert!(der.windows(13).any(|window| window == b"350101000000Z"));
        assert!(!der.windows(4).any(|window| window == b"4096"));
    }
}
