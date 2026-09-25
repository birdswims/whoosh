use std::sync::{Arc, Mutex, Once};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const ALPN: &[u8] = b"whoosh/1";
pub const SERVER_NAME: &str = "whoosh";

pub struct IdentityCert {
    pub cert_der: CertificateDer<'static>,
    pub key_der: PrivateKeyDer<'static>,
    pub fingerprint: [u8; 32],
}

pub fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn generate_identity() -> Result<IdentityCert> {
    install_crypto();
    let key_pair = rcgen::KeyPair::generate().map_err(|error| Error::crypto(error.to_string()))?;
    let params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])
        .map_err(|error| Error::crypto(error.to_string()))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let cert_der = CertificateDer::from(cert);
    let fingerprint = fingerprint(cert_der.as_ref());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    Ok(IdentityCert {
        cert_der,
        key_der,
        fingerprint,
    })
}

pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    Sha256::digest(der).into()
}

pub fn fingerprint_hex(bytes: &[u8; 32]) -> String {
    hex::encode(bytes)
}

#[derive(Debug)]
struct FingerprintVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
    expected: Option<[u8; 32]>,
    seen: Arc<Mutex<Option<[u8; 32]>>>,
}

impl FingerprintVerifier {
    fn new(expected: Option<[u8; 32]>, seen: Arc<Mutex<Option<[u8; 32]>>>) -> Arc<Self> {
        Arc::new(Self {
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            expected,
            seen,
        })
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let digest = fingerprint(end_entity.as_ref());
        *self.seen.lock().expect("fingerprint lock") = Some(digest);
        if let Some(expected) = self.expected {
            if digest != expected {
                return Err(rustls::Error::General(format!(
                    "certificate fingerprint {} does not match {}",
                    fingerprint_hex(&digest),
                    fingerprint_hex(&expected)
                )));
            }
        }
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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn fast_transport() -> quinn::TransportConfig {
    let mut transport = quinn::TransportConfig::default();
    transport
        .max_concurrent_uni_streams(32u32.into())
        .max_concurrent_bidi_streams(8u32.into())
        .stream_receive_window((8 * 1024 * 1024u32).into())
        .receive_window((32 * 1024 * 1024u32).into())
        .send_window(32 * 1024 * 1024)
        .keep_alive_interval(Some(std::time::Duration::from_secs(5)))
        .max_idle_timeout(Some(
            quinn::IdleTimeout::try_from(std::time::Duration::from_secs(120))
                .expect("idle timeout fits"),
        ))
        .congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    transport
}

pub fn server_config(identity: &IdentityCert) -> Result<quinn::ServerConfig> {
    install_crypto();
    let mut rustls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![identity.cert_der.clone()],
            identity.key_der.clone_key(),
        )
        .map_err(|error| Error::crypto(error.to_string()))?;
    rustls_config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(rustls_config)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    server.transport_config(Arc::new(fast_transport()));
    Ok(server)
}

/// How the sender decides the receiver certificate is the device it meant to reach.
pub enum Trust {
    /// Standard path validation against one self-signed certificate.
    Roots(CertificateDer<'static>),
    /// Trust on first use, or a previously pinned SHA-256 of the certificate.
    Fingerprint(Option<[u8; 32]>),
}

pub struct ClientCrypto {
    pub config: quinn::ClientConfig,
    pub seen: Arc<Mutex<Option<[u8; 32]>>>,
}

pub fn client_config(trust: Trust) -> Result<ClientCrypto> {
    install_crypto();
    let seen = Arc::new(Mutex::new(None));
    let mut rustls_config = match trust {
        Trust::Roots(cert) => {
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(cert)
                .map_err(|error| Error::crypto(error.to_string()))?;
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
        Trust::Fingerprint(expected) => rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(FingerprintVerifier::new(expected, seen.clone()))
            .with_no_client_auth(),
    };
    rustls_config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
        .map_err(|error| Error::crypto(error.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(fast_transport()));
    Ok(ClientCrypto { config, seen })
}
