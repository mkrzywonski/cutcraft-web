//! The Pi's identity (cameo-pi spec §14.4): its certificate is self-signed, so it's trusted on
//! first use and pinned. Afterwards exactly that certificate is accepted, whatever name or address
//! the Pi is reached by. The handshake signatures are still checked, with rustls's own ring
//! verifiers.

use std::fmt;
use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// A certificate's SHA-256, written as the Pi's `cameo-pi fingerprint` prints it: upper-case hex
/// pairs separated by colons.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(certificate: &[u8]) -> Self {
        Self(Sha256::digest(certificate).into())
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut bytes = [0u8; 32];
        let mut parts = text.trim().split(':');
        for byte in &mut bytes {
            let part = parts.next()?;
            if part.len() != 2 {
                return None;
            }
            *byte = u8::from_str_radix(part, 16).ok()?;
        }
        parts.next().is_none().then_some(Self(bytes))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Accepts the pinned certificate only, or (first contact, nothing pinned) any certificate,
/// remembering which.
#[derive(Debug)]
pub(crate) struct Pinned {
    expected: Option<Fingerprint>,
    /// The certificate the last handshake presented.
    pub(crate) seen: Mutex<Option<Fingerprint>>,
    algorithms: WebPkiSupportedAlgorithms,
}

/// The message a changed certificate fails with; [`crate::Error`] recognises it.
pub(crate) const CHANGED: &str = "cameo-pi's identity changed";

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = Fingerprint::of(end_entity);
        if let Ok(mut last) = self.seen.lock() {
            *last = Some(seen);
        }
        match self.expected {
            Some(expected) if expected != seen => Err(rustls::Error::General(CHANGED.into())),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// A client configuration that trusts `expected` only (or, without it, whatever is presented
/// first), and the verifier, to read which certificate was seen.
pub(crate) fn config(
    expected: Option<Fingerprint>,
) -> Result<(rustls::ClientConfig, Arc<Pinned>), rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Pinned {
        expected,
        seen: Mutex::new(None),
        algorithms: provider.signature_verification_algorithms,
    });
    let config = rustls::ClientConfig::builder_with_provider(provider as Arc<CryptoProvider>)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    Ok((config, verifier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_reads_back_as_the_pi_prints_it() {
        let f = Fingerprint::of(b"certificate");
        let text = f.to_string();
        assert_eq!(text.len(), 32 * 3 - 1);
        assert!(
            text.chars()
                .all(|c| c == ':' || c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
        );
        assert_eq!(Fingerprint::parse(&text), Some(f));
        assert_eq!(Fingerprint::parse(&text.to_lowercase()), Some(f));
        assert_eq!(Fingerprint::parse(&text[3..]), None);
        assert_eq!(Fingerprint::parse(&format!("{text}:00")), None);
    }
}
