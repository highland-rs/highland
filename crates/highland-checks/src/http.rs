// Rust guideline compliant 2026-09-27

//! The HTTP probe: does this URL answer with the status the operator expects?
//!
//! Hand-rolled over TCP, deliberately. An HTTP health check is a request line, a
//! status line, and a decision; a client library brings a connection pool, a
//! redirect policy, a cookie jar, and a TLS stack, none of which a health check
//! should have opinions about. The pieces that matter for a check are here:
//!
//! - the response body is bounded, because a health check that reads an
//!   unbounded body from a service that is misbehaving is how a monitor becomes
//!   an amplifier;
//! - the headers are read, because the check is about the answer, not about the
//!   connection succeeding;
//! - only the status line decides, so a `200` from a proxy and a `200` from the
//!   service are equally acceptable, and a `503` is a failure whichever it came
//!   from.
//!
//! `https` is not implemented here. A TLS handshake is not something to
//! reimplement, and a check that connected to port 443 without validating a
//! certificate would be a check that reports an endpoint as healthy when its
//! identity is wrong — worse than no check at all.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use highland_core::state::Generation;

use crate::check::{Check, CheckKind, CheckSpec};
use crate::error::{CheckError, Result};
use crate::result::CheckResult;

/// The most a health check will read from a response, headers included.
///
/// A megabyte is far more than a status line and generous to a service that
/// sends a small body, and small enough that a service streaming an error page
/// cannot exhaust the check's memory on every interval.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The parts of a URL a probe needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// The scheme, lowercased.
    pub scheme: String,
    /// The host to connect to.
    pub host: String,
    /// The port, defaulted from the scheme.
    pub port: u16,
    /// The path and query, defaulted to `/`.
    pub target: String,
}

impl Url {
    /// Parses `url`, rejecting anything this check cannot do honestly.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::UnresolvableTarget`] for a URL that is not
    /// `http://host[:port][/path]`, and for `https`, which is not implemented.
    /// A `https` URL is refused rather than downgraded: connecting to port 443
    /// without a TLS handshake would report a service as healthy on the strength
    /// of a plaintext exchange.
    pub fn parse(check: &str, url: &str) -> Result<Self> {
        let (scheme, rest) =
            url.split_once("://")
                .ok_or_else(|| CheckError::UnresolvableTarget {
                    check: check.to_owned(),
                    target: url.to_owned(),
                    detail: "a URL needs a scheme, like http://192.0.2.10:8080/health".to_owned(),
                })?;
        let scheme = scheme.to_ascii_lowercase();
        if scheme == "https" {
            return Err(CheckError::UnresolvableTarget {
                check: check.to_owned(),
                target: url.to_owned(),
                detail: "an https check needs TLS, which this build does not implement; \\
                         use a tcp check on the same port rather than a check that cannot \\
                         validate a certificate"
                    .to_owned(),
            });
        }
        if scheme != "http" {
            return Err(CheckError::UnresolvableTarget {
                check: check.to_owned(),
                target: url.to_owned(),
                detail: format!("{scheme} is not a scheme this check speaks"),
            });
        }

        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_owned()),
        };
        if authority.is_empty() {
            return Err(CheckError::UnresolvableTarget {
                check: check.to_owned(),
                target: url.to_owned(),
                detail: "the URL has no host".to_owned(),
            });
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => {
                let port: u16 = port.parse().map_err(|_| CheckError::UnresolvableTarget {
                    check: check.to_owned(),
                    target: url.to_owned(),
                    detail: format!("{port} is not a port number"),
                })?;
                (host.to_owned(), port)
            }
            None => (authority.to_owned(), 80),
        };
        Ok(Self {
            scheme,
            host,
            port,
            target: path,
        })
    }

    /// The `Host` header value for this URL.
    #[must_use]
    pub fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// A check that requests a URL and validates the status line.
#[derive(Debug, Clone)]
pub struct HttpCheck {
    spec: CheckSpec,
    url: Url,
    expected: Vec<u16>,
}

impl HttpCheck {
    /// Creates a check for `url`, accepting the listed status codes.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::UnresolvableTarget`] when the URL cannot be used.
    pub fn new(spec: CheckSpec, url: &str, expected: Vec<u16>) -> Result<Self> {
        debug_assert!(matches!(spec.kind, CheckKind::Http | CheckKind::Https));
        let parsed = Url::parse(&spec.name, url)?;
        // An empty list would mean "accept anything", which is not a check: it
        // passes while the service returns 500. The default is what an operator
        // writing a health check almost always means.
        let expected = if expected.is_empty() {
            vec![200]
        } else {
            expected
        };
        Ok(Self {
            spec,
            url: parsed,
            expected,
        })
    }

    /// The parsed URL this check requests.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }
}

/// One HTTP response, as far as a health check cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code from the status line.
    pub status: u16,
    /// The reason phrase, when the server sent one.
    pub reason: String,
}

/// Parses a status line out of a response's first line.
///
/// # Errors
///
/// Returns [`CheckError::UnresolvableTarget`] when the first line is not a
/// status line, which is what a service that is not an HTTP service says.
pub fn parse_status_line(check: &str, line: &str) -> Result<Response> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err(CheckError::UnresolvableTarget {
            check: check.to_owned(),
            target: line.to_owned(),
            detail: "the response did not begin with a status line".to_owned(),
        });
    }
    let status: u16 =
        parts
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| CheckError::UnresolvableTarget {
                check: check.to_owned(),
                target: line.to_owned(),
                detail: "the status code is not a number".to_owned(),
            })?;
    Ok(Response {
        status,
        reason: parts.next().unwrap_or_default().trim().to_owned(),
    })
}

impl Check for HttpCheck {
    fn spec(&self) -> &CheckSpec {
        &self.spec
    }

    fn run(
        &self,
        generation: Generation,
        sequence: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<CheckResult>> + Send + '_>> {
        Box::pin(self.probe(generation, sequence))
    }
}

impl HttpCheck {
    /// The probe itself, which [`Check::run`] boxes.
    async fn probe(&self, generation: Generation, sequence: u64) -> Result<CheckResult> {
        let started = Instant::now();
        let name = self.spec.name.clone();
        let url = self.url.clone();
        let expected = self.expected.clone();
        // The probe is synchronous inside an async function, so it is pushed to
        // the blocking pool: a check must never occupy a runtime worker while it
        // waits on a socket (`R-05`, `I-38`).
        let requested = url.clone();
        let outcome = tokio::task::spawn_blocking(move || request(&requested)).await;

        match outcome {
            Ok(Ok(response)) => {
                if expected.contains(&response.status) {
                    CheckResult::passing(
                        name,
                        started.elapsed(),
                        Instant::now(),
                        generation,
                        sequence,
                        format!(
                            "{} answered {} {}",
                            url.host, response.status, response.reason
                        ),
                    )
                } else {
                    CheckResult::failing(
                        name,
                        None,
                        Instant::now(),
                        generation,
                        sequence,
                        format!(
                            "{} answered {} {}, expected one of {expected:?}",
                            url.host, response.status, response.reason
                        ),
                    )
                }
            }
            Ok(Err(error)) => CheckResult::failing(
                name,
                None,
                Instant::now(),
                generation,
                sequence,
                format!("{}: {error}", url.host),
            ),
            Err(error) => CheckResult::failing(
                name,
                None,
                Instant::now(),
                generation,
                sequence,
                format!("the probe task did not finish: {error}"),
            ),
        }
    }
}

/// Performs one request and returns the response's status line.
///
/// The body is read up to [`MAX_RESPONSE_BYTES`] and then dropped, so a service
/// that streams without end costs one bounded buffer rather than the check's
/// whole memory.
fn request(url: &Url) -> std::result::Result<Response, String> {
    let addresses = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|error| format!("could not resolve the host: {error}"))?;
    let address = addresses
        .into_iter()
        .next()
        .ok_or_else(|| "the host resolved to no addresses".to_owned())?;

    let mut stream = connect_bounded(address, Duration::from_secs(5))
        .map_err(|error| format!("could not connect: {error}"))?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: highland-check\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.target,
        url.host_header()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("could not send the request: {error}"))?;

    let mut response = Vec::new();
    Read::by_ref(&mut stream)
        .take(MAX_RESPONSE_BYTES as u64)
        .read_to_end(&mut response)
        .map_err(|error| format!("could not read the response: {error}"))?;

    let first_line = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    if first_line.is_empty() {
        return Err("the response was empty".to_owned());
    }
    parse_status_line(&url.host, &first_line).map_err(|error| error.to_string())
}

/// Connects with a bound, because a check that waits on an unreachable address
/// is a check that has stopped checking anything else.
fn connect_bounded(
    address: std::net::SocketAddr,
    budget: Duration,
) -> std::result::Result<TcpStream, String> {
    TcpStream::connect_timeout(&address, budget).map_err(|error| error.to_string())
}
