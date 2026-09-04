//! Client TLS towards the hub: system roots (`webpki`), an SPKI pin (`pinned`), or a
//! learning verifier used once during enrollment to decide which of the two applies.

use crate::world::crypto::sha256;
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;

/// What was observed about the hub's certificate during an enrollment connection.
#[derive(Clone, Debug)]
pub(crate) struct Learned {
    /// SPKI SHA-256 of the leaf certificate, hex.
    pub(crate) spki_sha256: String,
    /// Whether the chain verified against the system roots for the name we dialed.
    pub(crate) webpki_ok: bool,
    pub(crate) webpki_error: Option<String>,
}

/// How to verify the hub's certificate.
#[derive(Clone)]
pub(crate) enum Verify {
    Webpki,
    Pinned(String),
    /// Accept any certificate and record what was seen; enrollment only.
    Learn(Arc<Mutex<Option<Learned>>>),
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn root_store() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for certificate in native.certs {
        let _ = roots.add(certificate);
    }
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    roots
}

/// Builds a TLS 1.3 client configuration for one verification mode.
pub(crate) fn client_config(verify: &Verify) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| format!("Cannot configure TLS 1.3: {error}"))?;
    let mut config = match verify {
        Verify::Webpki => builder
            .with_root_certificates(root_store())
            .with_no_client_auth(),
        Verify::Pinned(spki) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier {
                spki: spki.to_ascii_lowercase(),
                provider,
            }))
            .with_no_client_auth(),
        Verify::Learn(slot) => {
            let webpki = rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(root_store()),
                Arc::clone(&provider),
            )
            .build()
            .map_err(|error| format!("Cannot build the certificate verifier: {error}"))?;
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(LearningVerifier {
                    webpki,
                    slot: Arc::clone(slot),
                }))
                .with_no_client_auth()
        }
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

pub(crate) fn spki_sha256(certificate: &CertificateDer<'_>) -> Result<String, rustls::Error> {
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate.as_ref())
        .map_err(|_| rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding))?;
    Ok(hex::encode(sha256(parsed.tbs_certificate.subject_pki.raw)))
}

/// Accepts exactly the certificate whose public key was pinned at enrollment.
#[derive(Debug)]
struct PinVerifier {
    spki: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = spki_sha256(end_entity)?;
        if presented == self.spki {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "hub_identity_mismatch: the hub presented TLS key {presented}, not the pinned {}",
                self.spki
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
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
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
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

/// Records the leaf SPKI and whether the chain verified, then accepts. The application
/// handshake that follows (hub signature with channel binding) is what actually
/// authenticates the hub during enrollment.
#[derive(Debug)]
struct LearningVerifier {
    webpki: Arc<rustls::client::WebPkiServerVerifier>,
    slot: Arc<Mutex<Option<Learned>>>,
}

impl ServerCertVerifier for LearningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let spki_sha256 = spki_sha256(end_entity)?;
        let outcome = self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        *self.slot.lock() = Some(Learned {
            spki_sha256,
            webpki_ok: outcome.is_ok(),
            webpki_error: outcome.err().map(|error| error.to_string()),
        });
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}
