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
    let key_pair = rcgen::KeyPair::generate().map_err(|error| Error::crypto(error.to_string()))?;
    let params = rcgen::CertificateParams::new(vec!["AirDrop".into()])
        .map_err(|error| Error::crypto(error.to_string()))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let cert_der = CertificateDer::from(cert);
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der)
        .map_err(|error| Error::crypto(error.to_string()))?;
    Ok((TlsAcceptor::from(Arc::new(config)), cert_der))
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
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(SignatureOnly::new())
        .with_no_client_auth();
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
