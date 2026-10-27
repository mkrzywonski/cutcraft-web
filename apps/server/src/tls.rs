//! HTTPS like a home router (spec §14.4): a self-signed certificate made by the Pi on first start
//! and kept with its private state (never in the library or its backups). Nothing else is done
//! with certificates: no upload, renewal or management.

use std::path::Path;
use std::sync::Arc;

use cameo_pi_core::journal::write_atomic;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

/// Years the certificate is valid.
const YEARS: i32 = 10;

pub struct Identity {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

impl Identity {
    /// SHA-256 of the certificate, as `AB:CD:…`: what programs pin and `cameo-pi fingerprint`
    /// prints.
    pub fn fingerprint(&self) -> String {
        Sha256::digest(&self.cert)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

fn this_year() -> i32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    1970 + i32::try_from(secs / 31_556_952).unwrap_or(56)
}

/// The certificate in `private/tls`, made on first use for `names` (host names and IP
/// addresses).
pub fn load_or_create(private: &Path, names: &[String]) -> Result<Identity, String> {
    let dir = private.join("tls");
    let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        return Ok(Identity { cert, key });
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut params = rcgen::CertificateParams::new(names.to_vec()).map_err(|e| e.to_string())?;
    let year = this_year();
    params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    params.not_after = rcgen::date_time_ymd(year + YEARS, 1, 1);
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, "cameo-pi");
    params.distinguished_name = subject;
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    let identity = Identity {
        cert: cert.der().to_vec(),
        key: key.serialize_der(),
    };
    write_atomic(&key_path, &identity.key).map_err(|e| e.to_string())?;
    restrict(&key_path)?;
    write_atomic(&cert_path, &identity.cert).map_err(|e| e.to_string())?;
    Ok(identity)
}

/// Only cameo-pi's own user may read the private key.
fn restrict(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())
}

/// The TLS server configuration for the certificate.
pub fn server_config(identity: &Identity) -> Result<Arc<rustls::ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.cert.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key.clone())),
        )
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_is_made_once_and_kept_private() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["cameo-pi.local".to_string(), "192.0.2.20".to_string()];
        let first = load_or_create(dir.path(), &names).unwrap();
        let again = load_or_create(dir.path(), &names).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.fingerprint().len(), 32 * 3 - 1);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("tls/key.der"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        server_config(&first).unwrap();
    }
}
