//! Optional TLS for the local file server, plus the WebSocket dialer.
//!
//! There is no certificate authority on a LAN, so ShareLocal generates its own
//! self-signed certificate covering the local hostname and every current IP.
//! Peers accept it because the trust anchor here is the *transfer token* and
//! the PIN, not the certificate — TLS is used for confidentiality on the wire,
//! not for identity. That trade-off is why this is opt-in and off by default.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

pub struct TlsMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

fn cert_paths(data_dir: &Path) -> (PathBuf, PathBuf) {
    (data_dir.join("tls-cert.pem"), data_dir.join("tls-key.pem"))
}

/// Load the stored certificate, regenerating it when it is missing or no
/// longer covers our current addresses.
pub fn ensure_certificate(data_dir: &Path) -> anyhow::Result<TlsMaterial> {
    let (cert_path, key_path) = cert_paths(data_dir);
    let names = subject_names();

    if let (Ok(cert_pem), Ok(key_pem)) = (
        std::fs::read_to_string(&cert_path),
        std::fs::read_to_string(&key_path),
    ) {
        // A cheap staleness check: the stored certificate records the names it
        // was minted for in a sidecar file.
        let stored = std::fs::read_to_string(data_dir.join("tls-names.txt")).unwrap_or_default();
        if stored.trim() == names.join(",") && !cert_pem.is_empty() {
            return Ok(TlsMaterial { cert_pem, key_pem });
        }
    }

    let generated = rcgen::generate_simple_self_signed(names.clone())?;
    let cert_pem = generated.cert.pem();
    let key_pem = generated.signing_key.serialize_pem();

    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&cert_path, &cert_pem)?;
    std::fs::write(&key_path, &key_pem)?;
    std::fs::write(data_dir.join("tls-names.txt"), names.join(","))?;
    restrict_permissions(&key_path);

    Ok(TlsMaterial { cert_pem, key_pem })
}

/// Every name a peer might use to reach us.
fn subject_names() -> Vec<String> {
    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    for ip in crate::net::local_addresses() {
        names.push(ip.to_string());
    }
    names.sort();
    names.dedup();
    names
}

fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Certificate verification for LAN peers.
///
/// Deliberately permissive: every ShareLocal device presents a self-signed
/// certificate, so chain validation would fail for all of them. Authentication
/// is carried by the session token and the optional PIN instead. This verifier
/// is only ever used for connections we have already restricted to private
/// addresses.
#[derive(Debug)]
struct LanCertVerifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for LanCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
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
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn lan_client_config() -> Arc<rustls::ClientConfig> {
    let provider = rustls::crypto::ring::default_provider();
    let verifier = Arc::new(LanCertVerifier {
        provider: Arc::new(provider.clone()),
    });
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Arc::new(config)
}

/// Open the control channel to a peer, transparently handling `ws://` and the
/// self-signed `wss://` case.
pub async fn connect_ws(url: &str) -> anyhow::Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
    let (stream, _response) = if url.starts_with("wss://") {
        tokio_tungstenite::connect_async_tls_with_config(
            url,
            None,
            false,
            Some(Connector::Rustls(lan_client_config())),
        )
        .await?
    } else {
        tokio_tungstenite::connect_async(url).await?
    };
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_is_generated_and_then_reused() {
        let dir = std::env::temp_dir().join(format!("sl-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let first = ensure_certificate(&dir).expect("certificate generation works");
        assert!(first.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(first.key_pem.contains("PRIVATE KEY"));

        let second = ensure_certificate(&dir).expect("second call reuses the stored material");
        assert_eq!(first.cert_pem, second.cert_pem, "the certificate is stable");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stale_certificate_is_regenerated() {
        let dir = std::env::temp_dir().join(format!("sl-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = ensure_certificate(&dir).unwrap();

        // Simulate the machine having moved to a different network.
        std::fs::write(dir.join("tls-names.txt"), "old.example").unwrap();
        let second = ensure_certificate(&dir).unwrap();
        assert_ne!(
            first.cert_pem, second.cert_pem,
            "a certificate that no longer covers our addresses is replaced"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_private_key_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sl-tls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        ensure_certificate(&dir).unwrap();

        let (_, key_path) = cert_paths(&dir);
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the TLS key must be owner-only");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn subject_names_always_cover_loopback() {
        let names = subject_names();
        assert!(names.iter().any(|n| n == "localhost"));
        assert!(names.iter().any(|n| n == "127.0.0.1"));
    }
}
