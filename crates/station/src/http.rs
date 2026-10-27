//! HTTP/1.1 over the pinned TLS connection, on a small runtime of its own. Each request opens its
//! own connection, which is plenty for a few requests a second and leaves nothing to go stale.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use crate::tls::{self, Fingerprint, Pinned};
use crate::{Error, Result};

/// One request.
pub(crate) struct Request<'a> {
    pub method: &'static str,
    /// The path and query, `/api/v1/...`.
    pub path: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub bearer: Option<&'a str>,
    pub timeout: Duration,
}

impl Request<'_> {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: "GET",
            path: path.into(),
            content_type: None,
            body: Vec::new(),
            bearer: None,
            timeout: crate::TIMEOUT,
        }
    }

    pub fn json(path: impl Into<String>, body: &serde_json::Value) -> Self {
        Self {
            method: "POST",
            path: path.into(),
            content_type: Some("application/json".into()),
            body: body.to_string().into_bytes(),
            bearer: None,
            timeout: crate::TIMEOUT,
        }
    }
}

/// The connection details for one Pi.
pub(crate) struct Http {
    /// To connect to, without IPv6 brackets.
    host: String,
    port: u16,
    /// The Host header: the name or address as given, so the Pi's host check sees it.
    authority: String,
    pinned: Option<Fingerprint>,
    connector: TlsConnector,
    pub verifier: Arc<Pinned>,
    runtime: tokio::runtime::Runtime,
}

impl Http {
    /// For `https://AUTHORITY` (from [`crate::base_url`]), trusting `pinned` only, or without it
    /// whatever certificate comes first.
    pub fn new(base: &str, pinned: Option<Fingerprint>) -> Result<Self> {
        let authority = base.strip_prefix("https://").unwrap_or(base).to_owned();
        let (host, port) = split_authority(&authority)
            .ok_or_else(|| Error::Address(format!("{authority:?} isn't a cameo-pi address")))?;
        let (config, verifier) =
            tls::config(pinned).map_err(|e| Error::Protocol(format!("TLS set-up: {e}")))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|e| Error::Protocol(format!("runtime set-up: {e}")))?;
        Ok(Self {
            host,
            port,
            authority,
            pinned,
            connector: TlsConnector::from(Arc::new(config)),
            verifier,
            runtime,
        })
    }

    /// The certificate the last connection presented.
    pub fn seen(&self) -> Option<Fingerprint> {
        self.verifier.seen.lock().ok().and_then(|s| *s)
    }

    /// Sends `request` and returns the status and body.
    pub fn send(&self, request: Request<'_>) -> Result<(u16, Vec<u8>)> {
        let timeout = request.timeout;
        self.runtime.block_on(async {
            tokio::time::timeout(timeout, self.exchange(request))
                .await
                .unwrap_or_else(|_| Err(Error::Unreachable("it didn't answer in time".into())))
        })
    }

    async fn exchange(&self, request: Request<'_>) -> Result<(u16, Vec<u8>)> {
        let unreachable = |e: &dyn std::fmt::Display| Error::Unreachable(e.to_string());
        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| unreachable(&e))?;
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| Error::Address(format!("{:?} isn't a cameo-pi address", self.host)))?;
        let tls = self.connector.connect(name, tcp).await.map_err(|e| {
            match (self.pinned, self.seen()) {
                (Some(pinned), Some(seen)) if pinned != seen => {
                    Error::IdentityChanged { pinned, seen }
                }
                _ => unreachable(&e),
            }
        })?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|e| unreachable(&e))?;
        tokio::spawn(connection);
        let mut builder = hyper::Request::builder()
            .method(request.method)
            .uri(&request.path)
            .header(hyper::header::HOST, &self.authority)
            .header(
                hyper::header::USER_AGENT,
                concat!("CutCraft/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(kind) = &request.content_type {
            builder = builder.header(hyper::header::CONTENT_TYPE, kind);
        }
        if let Some(key) = request.bearer {
            builder = builder.header(hyper::header::AUTHORIZATION, format!("Bearer {key}"));
        }
        let message = builder
            .body(Full::new(Bytes::from(request.body)))
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let response = sender
            .send_request(message)
            .await
            .map_err(|e| unreachable(&e))?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| unreachable(&e))?
            .to_bytes();
        Ok((status, body.to_vec()))
    }
}

/// `host[:port]` or `[v6][:port]` into the host to connect to and the port (443 by default).
fn split_authority(authority: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return None,
            None => (authority, None),
        }
    };
    let port = match port {
        Some(port) => port.parse().ok()?,
        None => 443,
    };
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

/// Percent-encodes a query value.
pub(crate) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A `multipart/form-data` body: `(name, filename, bytes)` parts, a filename for file parts.
pub(crate) fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = format!("cutcraft-{}", crate::new_key());
    let mut body = Vec::new();
    for (name, file, bytes) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match file {
            Some(file) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities_split_into_host_and_port() {
        assert_eq!(
            split_authority("cameo-pi.local"),
            Some(("cameo-pi.local".into(), 443))
        );
        assert_eq!(
            split_authority("10.0.0.5:8443"),
            Some(("10.0.0.5".into(), 8443))
        );
        assert_eq!(
            split_authority("[fe80::1]:444"),
            Some(("fe80::1".into(), 444))
        );
        assert_eq!(split_authority("[fe80::1]"), Some(("fe80::1".into(), 443)));
        assert_eq!(split_authority("fe80::1"), None);
        assert_eq!(split_authority("pi:port"), None);
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(encode("T-shirts/Kids & Co"), "T-shirts/Kids%20%26%20Co");
        assert_eq!(encode("Été"), "%C3%89t%C3%A9");
    }
}
