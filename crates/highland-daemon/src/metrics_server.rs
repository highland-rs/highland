// Rust guideline compliant 2026-09-27

//! The Prometheus scrape endpoint.
//!
//! Deliberately not a web framework. A metrics endpoint needs to answer `GET /`
//! and nothing else, and a framework here would be the largest dependency in the
//! daemon for the sake of four lines of HTTP. The response is small, the parser
//! is smaller, and both are testable.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::control::StatusRegistry;
use crate::metrics::{CONTENT_TYPE, Metrics};

/// How long a client may take to send a request.
///
/// A scrape endpoint that can be made to hold a connection open is a way to
/// exhaust the daemon's tasks.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The largest request line a scrape endpoint will read.
const MAX_REQUEST: usize = 1024;

/// The scrape endpoint.
#[derive(Debug)]
pub struct MetricsServer {
    metrics: Arc<Metrics>,
    registry: Arc<StatusRegistry>,
    version: String,
}

/// A problem serving metrics, which never stops the daemon.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MetricsError {
    /// The listener could not be bound.
    #[error("could not listen on {address}: {reason}")]
    Listen {
        /// The address that was requested.
        address: SocketAddr,
        /// What the operating system reported.
        reason: String,
    },

    /// One connection failed.
    #[error("metrics connection failed: {reason}")]
    Connection {
        /// What went wrong.
        reason: String,
    },
}

impl MetricsServer {
    /// Creates the endpoint.
    #[must_use]
    pub fn new(
        metrics: Arc<Metrics>,
        registry: Arc<StatusRegistry>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            metrics,
            registry,
            version: version.into(),
        }
    }

    /// Binds the address and serves until the process ends.
    ///
    /// # Errors
    ///
    /// Returns [`MetricsError::Listen`] when the address cannot be bound. The
    /// caller decides whether that is fatal; it is not for the daemon, because
    /// metrics are for a scraper and forwarding addresses are for clients.
    pub async fn serve(self, address: SocketAddr) -> Result<(), MetricsError> {
        let listener = TcpListener::bind(address)
            .await
            .map_err(|error| MetricsError::Listen {
                address,
                reason: error.to_string(),
            })?;
        tracing::info!(%address, "metrics endpoint listening");

        // One listener, one shared handler: the handler is `Send` and cheap to
        // clone, so a scrape never borrows the accept loop.
        let handler = Arc::new(self);
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(error = %error, "metrics accept failed");
                    continue;
                }
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                if let Err(error) = handler.answer(stream).await {
                    tracing::debug!(%peer, error = %error, "metrics scrape ended");
                }
            });
        }
    }

    /// Answers one request.
    ///
    /// Split out so the protocol is testable without binding a port.
    ///
    /// # Errors
    ///
    /// Returns [`MetricsError::Connection`] when the connection fails.
    pub async fn answer(&self, mut stream: TcpStream) -> Result<(), MetricsError> {
        let request = read_request(&mut stream).await?;
        let response = if request.starts_with("GET /") || request.starts_with("HEAD /") {
            let body = crate::metrics::render(&self.metrics, &self.registry, &self.version);
            http_response("200 OK", CONTENT_TYPE, &body)
        } else {
            http_response(
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                "use GET\n",
            )
        };
        stream
            .write_all(response.as_bytes())
            .await
            .map_err(|error| MetricsError::Connection {
                reason: error.to_string(),
            })
    }
}

/// Reads a request line, bounded in both time and length.
async fn read_request(stream: &mut TcpStream) -> Result<String, MetricsError> {
    let mut buffer = vec![0_u8; MAX_REQUEST];
    let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut buffer))
        .await
        .map_err(|_| MetricsError::Connection {
            reason: "the client sent nothing".to_owned(),
        })?
        .map_err(|error| MetricsError::Connection {
            reason: error.to_string(),
        })?;

    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
    Ok(text.lines().next().unwrap_or_default().to_owned())
}

/// Builds an HTTP/1.1 response.
///
/// `Content-Length` is included because a scraper that cannot tell when the body
/// ends waits, and a metrics endpoint that makes Prometheus wait is a metrics
/// endpoint nobody scrapes.
fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
        length = body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::InstanceStatus;

    fn server() -> MetricsServer {
        let registry = StatusRegistry::new("node-a");
        registry.publish(InstanceStatus {
            name: "api".to_owned(),
            role: "MASTER".to_owned(),
            priority: 150,
            effective_priority: 150,
            owns_addresses: true,
            vip_addresses: vec!["192.0.2.100/24".to_owned()],
            peers: vec!["192.0.2.11".to_owned()],
            last_reason: "startup".to_owned(),
        });
        MetricsServer::new(Metrics::shared(), Arc::new(registry), "0.1.0")
    }

    #[tokio::test]
    async fn a_scrape_answers_with_the_metrics() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let address = listener.local_addr().expect("the address is known");
        let served = server();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("a connection");
            let _ = served.answer(stream).await;
        });

        let mut client = TcpStream::connect(address).await.expect("connects");
        client
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .expect("written");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .await
            .expect("readable");

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains(CONTENT_TYPE), "{response}");
        assert!(response.contains("highland_up 1"), "{response}");
        assert!(
            response.contains("highland_instance_role{instance=\"api\"} 2"),
            "{response}"
        );
        assert!(
            response.contains(&format!(
                "Content-Length: {}",
                response.split("\r\n\r\n").nth(1).map_or(0, str::len)
            )),
            "the length is stated so a scraper knows when the body ends: {response}"
        );
    }

    #[tokio::test]
    async fn a_method_other_than_get_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let address = listener.local_addr().expect("the address is known");
        let served = server();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("a connection");
            let _ = served.answer(stream).await;
        });

        let mut client = TcpStream::connect(address).await.expect("connects");
        client
            .write_all(b"POST /metrics HTTP/1.1\r\n\r\n")
            .await
            .expect("written");
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .await
            .expect("readable");

        assert!(response.starts_with("HTTP/1.1 405"), "{response}");
    }

    #[test]
    fn the_response_declares_its_length() {
        let response = http_response("200 OK", "text/plain", "a\nb");
        assert!(response.contains("Content-Length: 3"), "{response}");
        assert!(response.ends_with("a\nb"), "{response}");
    }
}
