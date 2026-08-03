//! The smallest HTTPS client that will do (`CON-004`, `CON-022`).
//!
//! Two Astra endpoints have to be called from this crate: the bundle's metadata service, over
//! mutual TLS with the bundle's own certificate, and the DevOps API, over ordinary public TLS
//! with a bearer token. Both are one request each, at start-up, returning small JSON documents.
//!
//! `AGENTS.md` reserves HTTP client crates for `cdm-api`, and the dependency graph in
//! `ARCHITECTURE.md` §3 has no edge from `cdm-cql` to one. That rule is worth keeping: the mTLS
//! call needs a rustls `ClientConfig` this crate has already built for the driver, which a
//! general-purpose client would only wrap again. So this module speaks HTTP/1.1 directly over
//! [`tokio_rustls`]: a request line, headers, `Connection: close`, and a response body that is
//! either `Content-Length`-delimited, `chunked`, or read to end-of-stream.
//!
//! It is deliberately not a general HTTP client. There is no redirect following, no compression,
//! no connection reuse and no HTTP/2 — none of which either endpoint needs.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use cdm_core::{CdmError, Side};
use rustls::pki_types::ServerName;
use rustls::ClientConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::errors::{connect_error, connect_error_from};

/// How long a single request may take, end to end. Astra's endpoints answer in milliseconds; a
/// request that has not finished in thirty seconds is a network problem, and saying so beats
/// hanging a migration's start-up indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest response body accepted, so that a misdirected request cannot exhaust memory.
const MAX_BODY: usize = 8 * 1024 * 1024;

/// What an endpoint answered.
#[derive(Debug, Clone)]
pub(crate) struct HttpResponse {
    /// The HTTP status code.
    pub(crate) status: u16,
    /// The response body.
    pub(crate) body: Vec<u8>,
}

impl HttpResponse {
    /// Whether the status is in the 2xx range.
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as text, lossily, for error messages.
    pub(crate) fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// One HTTPS request.
#[derive(Debug)]
pub(crate) struct HttpRequest<'a> {
    /// `GET` or `POST`.
    pub(crate) method: &'a str,
    /// The hostname to connect to, and to send as `Host`.
    pub(crate) host: &'a str,
    /// The port to connect to.
    pub(crate) port: u16,
    /// The request target, e.g. `/metadata`.
    pub(crate) path: &'a str,
    /// Extra headers.
    pub(crate) headers: &'a [(&'a str, String)],
    /// The request body, if any.
    pub(crate) body: Option<&'a [u8]>,
}

/// Performs one HTTPS request with the given TLS configuration.
pub(crate) async fn send(
    side: Side,
    tls: Arc<ClientConfig>,
    request: HttpRequest<'_>,
) -> Result<HttpResponse, CdmError> {
    let target = format!("{}:{}", request.host, request.port);
    let server_name = ServerName::try_from(request.host.to_owned()).map_err(|e| {
        connect_error_from(side, format!("{} is not a valid hostname", request.host), e)
    })?;

    let exchange =
        async {
            let stream = TcpStream::connect(&target)
                .await
                .map_err(|e| connect_error_from(side, format!("cannot reach {target}"), e))?;
            let connector = TlsConnector::from(tls);
            let mut stream = connector.connect(server_name, stream).await.map_err(|e| {
                connect_error_from(side, format!("TLS handshake with {target} failed"), e)
            })?;

            let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: cdm-rs/{}\r\nAccept: application/json\r\n\
             Connection: close\r\n",
            request.method,
            request.path,
            request.host,
            crate::VERSION
        );
            for (name, value) in request.headers {
                // Writing into a `String` cannot fail; the result is discarded deliberately.
                let _ = write!(head, "{name}: {value}\r\n");
            }
            if let Some(body) = request.body {
                let _ = write!(head, "Content-Length: {}\r\n", body.len());
            }
            head.push_str("\r\n");

            stream
                .write_all(head.as_bytes())
                .await
                .map_err(|e| connect_error_from(side, format!("cannot write to {target}"), e))?;
            if let Some(body) = request.body {
                stream.write_all(body).await.map_err(|e| {
                    connect_error_from(side, format!("cannot write to {target}"), e)
                })?;
            }
            stream
                .flush()
                .await
                .map_err(|e| connect_error_from(side, format!("cannot write to {target}"), e))?;

            let mut raw = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let read = stream.read(&mut buffer).await.map_err(|e| {
                    connect_error_from(side, format!("cannot read from {target}"), e)
                })?;
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(buffer.get(..read).unwrap_or_default());
                if raw.len() > MAX_BODY {
                    return Err(connect_error(
                        side,
                        format!("the response from {target} exceeds {MAX_BODY} bytes"),
                    ));
                }
            }
            Ok::<Vec<u8>, CdmError>(raw)
        };

    let raw = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
        .await
        .map_err(|_| {
            connect_error(
                side,
                format!("{target} did not answer within {REQUEST_TIMEOUT:?}"),
            )
        })??;

    parse_response(side, &raw)
}

/// Splits an HTTP/1.1 response into its status and body, undoing chunked framing.
pub(crate) fn parse_response(side: Side, raw: &[u8]) -> Result<HttpResponse, CdmError> {
    let split = find(raw, b"\r\n\r\n")
        .ok_or_else(|| connect_error(side, "the HTTP response has no header terminator"))?;
    let head = String::from_utf8_lossy(raw.get(..split).unwrap_or_default()).into_owned();
    let body = raw.get(split + 4..).unwrap_or_default();

    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| connect_error(side, "the HTTP response has no status line"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            connect_error(
                side,
                format!("the HTTP status line is malformed: {status_line}"),
            )
        })?;

    let mut chunked = false;
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if name == "transfer-encoding" && value.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if name == "content-length" {
                content_length = value.parse::<usize>().ok();
            }
        }
    }

    let body = if chunked {
        dechunk(side, body)?
    } else if let Some(length) = content_length {
        body.get(..length.min(body.len()))
            .unwrap_or_default()
            .to_vec()
    } else {
        body.to_vec()
    };

    Ok(HttpResponse { status, body })
}

/// Reassembles a `Transfer-Encoding: chunked` body.
fn dechunk(side: Side, mut body: &[u8]) -> Result<Vec<u8>, CdmError> {
    let malformed = || connect_error(side, "the chunked HTTP response is malformed");
    let mut out = Vec::new();
    loop {
        let line_end = find(body, b"\r\n").ok_or_else(malformed)?;
        let header = String::from_utf8_lossy(body.get(..line_end).unwrap_or_default()).into_owned();
        let size_text = header.split(';').next().unwrap_or_default().trim();
        let chunk_len = usize::from_str_radix(size_text, 16).map_err(|_| malformed())?;
        body = body.get(line_end + 2..).ok_or_else(malformed)?;
        if chunk_len == 0 {
            return Ok(out);
        }
        let chunk = body.get(..chunk_len).ok_or_else(malformed)?;
        out.extend_from_slice(chunk);
        body = body.get(chunk_len + 2..).unwrap_or_default();
    }
}

/// The index of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn con_022_a_content_length_response_is_parsed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"hello\":1}trailing garbage";
        let response = parse_response(Side::Origin, raw).unwrap();
        assert_eq!(response.status, 200);
        assert!(response.is_success());
        assert_eq!(response.body_text(), "{\"hello\":1}");
    }

    #[test]
    fn con_022_a_chunked_response_is_reassembled() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n7\r\n, world\r\n0\r\n\r\n";
        let response = parse_response(Side::Origin, raw).unwrap();
        assert_eq!(response.body_text(), "hello, world");
    }

    #[test]
    fn con_022_a_response_without_a_length_is_read_to_the_end() {
        let raw = b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom";
        let response = parse_response(Side::Origin, raw).unwrap();
        assert_eq!(response.status, 500);
        assert!(!response.is_success());
        assert_eq!(response.body_text(), "boom");
    }

    #[test]
    fn con_022_a_malformed_response_is_an_error_not_a_panic() {
        assert!(parse_response(Side::Origin, b"nonsense").is_err());
        assert!(parse_response(Side::Origin, b"HTTP/1.1 wat OK\r\n\r\n").is_err());
        let truncated = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";
        assert!(parse_response(Side::Origin, truncated).is_err());
        let bad_size = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n";
        assert!(parse_response(Side::Origin, bad_size).is_err());
    }

    /// A one-shot HTTPS server that returns `response` and hands back the request it received.
    ///
    /// This is what makes the success path of [`send`] testable at all: everything else in this
    /// module is a parser, and the request cdm-rs actually puts on the wire — the request line,
    /// the `Host` header, the bearer token, `Connection: close` — is only visible here.
    async fn serve_once(
        pki: &crate::testfixtures::Pki,
        response: &'static [u8],
    ) -> (u16, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![pki.server_cert_der()], pki.server_key_der())
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            // The body arrives in its own TLS record, so read until the peer goes quiet rather
            // than assuming one `read` sees the whole request.
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    stream.read(&mut buffer),
                )
                .await;
                match read {
                    Ok(Ok(0)) | Err(_) => break,
                    Ok(Ok(n)) => request.extend_from_slice(&buffer[..n]),
                    Ok(Err(e)) => panic!("reading the request failed: {e}"),
                }
            }
            stream.write_all(response).await.unwrap();
            stream.shutdown().await.unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (port, handle)
    }

    fn client_config(pki: &crate::testfixtures::Pki) -> Arc<ClientConfig> {
        let trust = crate::tls::parse_trust_store(
            Side::Origin,
            pki.ca_pem().as_bytes(),
            None,
            crate::tls::StoreFormat::Pem,
        )
        .unwrap();
        crate::tls::TlsSpec::new(Side::Origin, trust)
            .client_config()
            .unwrap()
    }

    #[tokio::test]
    async fn con_022_a_request_is_sent_and_its_response_read() {
        let pki = crate::testfixtures::Pki::new();
        let (port, server) = serve_once(
            &pki,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
        )
        .await;

        let response = send(
            Side::Origin,
            client_config(&pki),
            HttpRequest {
                method: "GET",
                host: "localhost",
                port,
                path: "/metadata",
                headers: &[],
                body: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body_text(), "{}");

        let request = server.await.unwrap();
        assert!(
            request.starts_with("GET /metadata HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Host: localhost\r\n"), "{request}");
        assert!(request.contains("Connection: close\r\n"), "{request}");
        assert!(request.contains("cdm-rs/"), "{request}");
    }

    #[tokio::test]
    async fn con_004_a_post_carries_its_headers_and_body() {
        let pki = crate::testfixtures::Pki::new();
        let (port, server) = serve_once(
            &pki,
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\n\r\nnope!",
        )
        .await;

        let response = send(
            Side::Target,
            client_config(&pki),
            HttpRequest {
                method: "POST",
                host: "localhost",
                port,
                path: "/v2/databases/abc/secureBundleURL?all=true",
                headers: &[("Authorization", "Bearer AstraCS:secret".to_owned())],
                body: Some(b"{}"),
            },
        )
        .await
        .unwrap();

        assert_eq!(response.status, 401);
        assert!(!response.is_success());
        assert_eq!(response.body_text(), "nope!");

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v2/databases/abc/secureBundleURL?all=true"));
        assert!(request.contains("Authorization: Bearer AstraCS:secret\r\n"));
        assert!(request.contains("Content-Length: 2\r\n"));
        assert!(
            request.ends_with("{}"),
            "the body must follow the headers: {request}"
        );
    }

    #[tokio::test]
    async fn con_022_a_server_the_trust_store_does_not_know_is_refused() {
        let stranger = crate::testfixtures::Pki::new();
        let (port, server) = serve_once(&stranger, b"HTTP/1.1 200 OK\r\n\r\n").await;

        let err = send(
            Side::Origin,
            client_config(&crate::testfixtures::Pki::new()),
            HttpRequest {
                method: "GET",
                host: "localhost",
                port,
                path: "/metadata",
                headers: &[],
                body: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("TLS handshake"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn con_004_an_unreachable_host_is_a_connect_error() {
        let tls = crate::tls::TlsSpec::new(Side::Origin, crate::tls::TrustMaterial::default())
            .client_config()
            .unwrap();
        let err = send(
            Side::Origin,
            tls,
            HttpRequest {
                method: "GET",
                // Reserved by RFC 6761 to never resolve.
                host: "metadata.invalid",
                port: 29080,
                path: "/metadata",
                headers: &[],
                body: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Connect);
    }

    #[tokio::test]
    async fn con_004_an_invalid_hostname_is_rejected_before_connecting() {
        let tls = crate::tls::TlsSpec::new(Side::Origin, crate::tls::TrustMaterial::default())
            .client_config()
            .unwrap();
        let err = send(
            Side::Origin,
            tls,
            HttpRequest {
                method: "GET",
                host: "not a hostname",
                port: 443,
                path: "/",
                headers: &[],
                body: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not a valid hostname"));
    }
}
