//! TLS between machines and the hub.
//!
//! The hub has one self-signed certificate, made when the hub is created and
//! never replaced; machines accept exactly that certificate, by the SHA-256
//! of its bytes, and nothing else. There is no certificate authority, no
//! domain name and no expiry to check. Only TLS 1.3 is spoken.
//!
//! The one exception is the first contact of a password login: the device
//! does not know the certificate yet, takes whichever the server presents,
//! and learns it only once the login proved the server is the hub
//! (`crate::login`).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::ids::CertPin;

pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The hub's certificate and its private key, as DER.
pub struct HubCertificate {
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
}

impl HubCertificate {
    /// A new self-signed certificate on a new P-256 key.
    pub fn generate() -> Result<Self, String> {
        Self::generate_for("fastctx-hub")
    }

    /// A new self-signed certificate on a new P-256 key, naming `name`.
    pub fn generate_for(name: &str) -> Result<Self, String> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|error| format!("making a TLS key: {error}"))?;
        let mut params = rcgen::CertificateParams::new(vec![name.to_string()])
            .map_err(|error| format!("making a certificate: {error}"))?;
        // Machines pin the certificate itself and never look at its dates;
        // these only keep other TLS tools from calling it expired.
        params.not_before = rcgen::date_time_ymd(2026, 1, 1);
        params.not_after = rcgen::date_time_ymd(4096, 1, 1);
        let certificate = params.self_signed(&key).map_err(|error| format!("making a certificate: {error}"))?;
        Ok(Self { certificate: certificate.der().to_vec(), key: key.serialize_der() })
    }

    pub fn pin(&self) -> CertPin {
        CertPin::of(&self.certificate)
    }

    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, String> {
        let key = PrivateKeyDer::try_from(self.key.clone()).map_err(|error| format!("the hub's TLS key: {error}"))?;
        let mut config = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| format!("configuring TLS 1.3: {error}"))?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(self.certificate.clone())], key)
            .map_err(|error| format!("the hub's certificate: {error}"))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

/// A client configuration that accepts the one certificate `pin` names, or,
/// without a pin, any certificate whose key signs the handshake.
pub fn client_config(pin: Option<CertPin>) -> Arc<rustls::ClientConfig> {
    let provider = provider();
    let mut config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("the ring provider supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { pin, provider }))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

#[derive(Debug)]
struct Pinned {
    pin: Option<CertPin>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = CertPin::of(end_entity.as_ref());
        match self.pin {
            None => Ok(ServerCertVerified::assertion()),
            Some(pin) if presented == pin => Ok(ServerCertVerified::assertion()),
            Some(pin) => Err(rustls::Error::General(format!(
                "the server presented the certificate {presented}, not the hub's {pin}; something other than the hub answered"
            ))),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not spoken".to_string()))
    }

    // The certificate is pinned, but the handshake must still prove the
    // server holds its key.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
