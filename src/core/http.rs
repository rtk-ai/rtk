//! Per-request settings shared by RTK's outbound HTTP calls.
//!
//! ureq takes a proxy from `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` (minus `NO_PROXY`).
//! A proxy on another machine cannot reach this machine's loopback, so every request
//! built here connects to `localhost` and loopback addresses directly, and calls to
//! the daemon RTK located itself (`daemon_bind`) go through [`direct_global_timeout`]
//! whatever address it is bound to.
//!
//! Known limitations of the proxy path:
//! - ureq reaches every target through a `CONNECT` tunnel, `http://` ones included,
//!   and proxies commonly allow `CONNECT` to port 443 only: a plain-HTTP server on
//!   another machine needs a `NO_PROXY` entry.
//! - `rtk task` treats a `base_url` as its daemon (and skips the proxy) when the URL
//!   merely contains `localhost`, `127.0.0.1`, `[::1]` or `0.0.0.0`; a `base_url`
//!   naming the daemon by a LAN address follows the proxy variables.
//! - SOCKS proxies (`socks5://` and the like) are not supported.
//! - ureq takes the first of `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` for every
//!   scheme, and matches `NO_PROXY` entries as written: no trimming around commas,
//!   no CIDR ranges.

use std::net::IpAddr;
use std::time::Duration;
use ureq::RequestBuilder;
use ureq::http::Uri;

/// The longest a connection may take to open, whatever the request's own deadline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Abandons the whole exchange after `timeout` (ureq's `timeout_global`), and the
/// connection after [`CONNECT_TIMEOUT`] if that comes first.
pub fn global_timeout<B>(request: RequestBuilder<B>, timeout: Duration) -> RequestBuilder<B> {
    skip_proxy_if_local(with_global_timeout(request, timeout))
}

/// [`global_timeout`], never through a proxy: for RTK's own daemon, which may be
/// bound to any of this machine's addresses.
pub fn direct_global_timeout<B>(
    request: RequestBuilder<B>,
    timeout: Duration,
) -> RequestBuilder<B> {
    direct(with_global_timeout(request, timeout))
}

fn with_global_timeout<B>(request: RequestBuilder<B>, timeout: Duration) -> RequestBuilder<B> {
    request
        .config()
        .timeout_global(Some(timeout))
        .timeout_connect(Some(timeout.min(CONNECT_TIMEOUT)))
        .build()
}

/// GET of a pipeline source, abandoned if the connection takes longer than
/// [`CONNECT_TIMEOUT`]; a slow body is left to finish.
#[cfg(feature = "engine")]
pub fn get(url: &str) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
    use anyhow::Context;
    // nosemgrep: ureq-outside-telemetry -- inbound GET of a pipeline source the user named (a URL, or the GitHub API for a repository)
    let request = ureq::get(url);
    connect_timeout(request, CONNECT_TIMEOUT)
        .call()
        .with_context(|| format!("GET {url}"))
}

/// Abandons the request if the connection takes longer than `timeout` (ureq's
/// `timeout_connect`); a slow body is left to finish.
#[cfg(feature = "engine")]
fn connect_timeout<B>(request: RequestBuilder<B>, timeout: Duration) -> RequestBuilder<B> {
    skip_proxy_if_local(request.config().timeout_connect(Some(timeout)).build())
}

/// The largest body RTK reads, once decompressed.
#[cfg(feature = "engine")]
pub const MAX_BODY: u64 = 10 * 1024 * 1024;

/// The body as text, at most `limit` bytes once decompressed, whatever its content
/// type: invalid UTF-8 becomes U+FFFD. ureq's own limit counts the bytes on the
/// wire, so a small gzip body could inflate far past it; and its `read_to_string`
/// decodes lossily for `text/*` only.
pub fn read_text_lossy(
    response: &mut ureq::http::Response<ureq::Body>,
    limit: u64,
) -> anyhow::Result<String> {
    use anyhow::Context;
    use std::io::Read;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("read body")?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("body larger than {limit} bytes");
    }
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// The body parsed as JSON, read through [`read_text_lossy`] with [`MAX_BODY`].
#[cfg(feature = "engine")]
pub fn read_json(
    response: &mut ureq::http::Response<ureq::Body>,
) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    let text = read_text_lossy(response, MAX_BODY)?;
    serde_json::from_str(&text).context("body is not JSON")
}

/// Never goes through a proxy.
fn direct<B>(request: RequestBuilder<B>) -> RequestBuilder<B> {
    request.config().proxy(None).build()
}

fn skip_proxy_if_local<B>(request: RequestBuilder<B>) -> RequestBuilder<B> {
    if request.uri_ref().is_some_and(is_local) {
        direct(request)
    } else {
        request
    }
}

fn is_local(uri: &Uri) -> bool {
    let Some(host) = uri.host() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| {
            // `0.0.0.0` / `[::]` is how a server bound to every interface reports
            // itself; `::ffff:127.0.0.1` is loopback spelled as IPv6.
            let ip = ip.to_canonical();
            ip.is_loopback() || ip.is_unspecified()
        })
}

/// A one-shot HTTP server on `127.0.0.1` for tests of code that sends requests.
#[cfg(test)]
pub(crate) mod test_server {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// Answers the first request with `200 OK` and `body`; the handle yields the raw
    /// request. Join it only after the client returned: a request that never arrives
    /// leaves the server blocked in `accept`.
    pub(crate) fn serve_once(body: impl Into<Vec<u8>>) -> (u16, JoinHandle<String>) {
        serve_once_with_status("200 OK", body)
    }

    /// [`serve_once`] answering with `status` (`"401 Unauthorized"`).
    pub(crate) fn serve_once_with_status(
        status: &'static str,
        body: impl Into<Vec<u8>>,
    ) -> (u16, JoinHandle<String>) {
        serve_once_with(status, "", body)
    }

    /// [`serve_once_with_status`] adding `headers`, each ending in `\r\n`.
    pub(crate) fn serve_once_with(
        status: &'static str,
        headers: &'static str,
        body: impl Into<Vec<u8>>,
    ) -> (u16, JoinHandle<String>) {
        let body = body.into();
        // nosemgrep: raw-socket-usage -- test-only stand-in for an HTTP endpoint
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !is_complete(&request) {
                let n = stream.read(&mut chunk).expect("read");
                assert!(n > 0, "client closed before sending the whole request");
                request.extend_from_slice(&chunk[..n]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\n\r\n",
                body.len()
            )
            .expect("write");
            // A client that stops at a body limit may hang up mid-body.
            let _ = stream.write_all(&body);
            String::from_utf8_lossy(&request).into_owned()
        });
        (port, server)
    }

    /// The port of a proxy that hangs up on every connection. Its listener stays
    /// bound for the rest of the run, so no parallel test can be handed the port.
    pub(crate) fn refused_port() -> u16 {
        // nosemgrep: raw-socket-usage -- test-only stand-in for an unusable proxy
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        port
    }

    fn is_complete(request: &[u8]) -> bool {
        let text = String::from_utf8_lossy(request);
        let Some(end) = text.find("\r\n\r\n") else {
            return false;
        };
        let length = text[..end]
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        request.len() >= end + 4 + length
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{refused_port, serve_once};
    use super::*;

    fn local(url: &str) -> bool {
        is_local(&url.parse::<Uri>().expect("valid test URI"))
    }

    #[test]
    fn local_hosts_are_recognised() {
        assert!(local("http://127.0.0.1:8787/health"));
        assert!(local("http://127.1.2.3/"));
        assert!(local("http://localhost:11434/v1"));
        assert!(local("http://LocalHost/"));
        assert!(local("http://[::1]:8787/health"));
        assert!(local("http://[::ffff:127.0.0.1]:8787/health"));
        assert!(local("http://0.0.0.0:8787/health"));
        assert!(local("http://[::]:8787/health"));
    }

    #[test]
    fn remote_hosts_are_not_local() {
        assert!(!local("https://api.anthropic.com/v1/messages"));
        assert!(!local("http://10.0.0.1/"));
        assert!(!local("http://localhost.example.com/"));
        assert!(!local("http://[::2]/"));
    }

    /// `0x7f.1` is `127.0.0.1` in the inet_aton shorthand, which glibc and musl
    /// resolve and the loopback check does not recognise: it stands in for this
    /// machine reached through a LAN address.
    #[cfg(target_os = "linux")]
    const NOT_LOOPBACK_LOOKING: &str = "0x7f.1";

    type Build = fn(
        RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> RequestBuilder<ureq::typestate::WithoutBody>;

    /// Sends, through an agent whose proxy refuses every connection, to a server
    /// reached as `host`: the call succeeds only if it skipped the proxy.
    fn call_past_refusing_proxy(host: &str, build: Build) -> Result<String, ureq::Error> {
        let proxy = format!("http://127.0.0.1:{}", refused_port());
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .proxy(Some(ureq::Proxy::new(&proxy).expect("proxy")))
                .build(),
        );
        let (port, server) = serve_once("ok");
        let url = format!("http://{host}:{port}/health");
        let body = build(agent.get(&url)).call()?.body_mut().read_to_string()?;
        server.join().expect("server");
        Ok(body)
    }

    fn assert_reaches_server_past_the_proxy(host: &str, build: Build) {
        let body = call_past_refusing_proxy(host, build).expect("direct connection");
        assert_eq!(body, "ok");
    }

    #[test]
    fn global_timeout_skips_the_proxy_for_loopback() {
        assert_reaches_server_past_the_proxy("127.0.0.1", |r| {
            global_timeout(r, Duration::from_secs(5))
        });
    }

    #[test]
    #[cfg(feature = "engine")]
    fn connect_timeout_skips_the_proxy_for_loopback() {
        assert_reaches_server_past_the_proxy("127.0.0.1", |r| {
            connect_timeout(r, Duration::from_secs(5))
        });
    }

    #[test]
    fn direct_skips_the_proxy() {
        assert_reaches_server_past_the_proxy("127.0.0.1", direct);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn direct_skips_the_proxy_for_a_host_that_is_not_loopback() {
        assert!(!local(&format!("http://{NOT_LOOPBACK_LOOKING}/")));
        assert_reaches_server_past_the_proxy(NOT_LOOPBACK_LOOKING, direct);
    }

    #[test]
    #[cfg(feature = "engine")]
    fn text_is_decoded_lossily_whatever_the_content_type() {
        // Latin-1 "café" served as JSON: the lone 0xE9 byte is not UTF-8.
        let (port, server) = serve_once(b"caf\xe9".as_slice());
        // nosemgrep: ureq-outside-telemetry -- test request to a local stand-in server
        let request = ureq::get(&format!("http://127.0.0.1:{port}/p.toml"));
        let mut response = direct(request).call().expect("call");
        server.join().expect("server");
        assert_eq!(
            read_text_lossy(&mut response, MAX_BODY).expect("body"),
            "caf\u{fffd}"
        );
    }

    /// The response a one-shot server gives with `headers` and `body`.
    fn fetch(headers: &'static str, body: Vec<u8>) -> ureq::http::Response<ureq::Body> {
        let (port, server) = super::test_server::serve_once_with("200 OK", headers, body);
        // nosemgrep: ureq-outside-telemetry -- test request to a local stand-in server
        let request = ureq::get(&format!("http://127.0.0.1:{port}/p"));
        let response = direct(request).call().expect("call");
        server.join().expect("server");
        response
    }

    #[test]
    fn body_limit_counts_decompressed_bytes() {
        use std::io::Write;
        let text = vec![b'x'; 4096];
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gzip.write_all(&text).expect("gzip");
        let gzip = gzip.finish().expect("gzip");
        assert!(
            gzip.len() < 1024,
            "the compressed body fits under the limit"
        );
        let headers = "content-encoding: gzip\r\n";

        let mut response = fetch(headers, gzip.clone());
        assert!(read_text_lossy(&mut response, 1024).is_err());

        let mut response = fetch(headers, gzip);
        let read = read_text_lossy(&mut response, 4096).expect("exactly at the limit");
        assert_eq!(read.len(), 4096);
    }

    #[test]
    #[cfg(feature = "engine")]
    fn get_reaches_loopback_despite_a_proxy() {
        let (port, server) = serve_once("ok");
        let proxy = format!("http://127.0.0.1:{}", refused_port());
        let result = temp_env::with_vars(
            [
                ("ALL_PROXY", Some(proxy.as_str())),
                ("NO_PROXY", None),
                ("no_proxy", None),
            ],
            || get(&format!("http://127.0.0.1:{port}/p.toml")),
        );
        let mut response = result.expect("direct connection");
        server.join().expect("server");
        assert_eq!(
            read_text_lossy(&mut response, MAX_BODY).expect("body"),
            "ok"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn other_hosts_keep_the_proxy() {
        let result = call_past_refusing_proxy(NOT_LOOPBACK_LOOKING, |r| {
            global_timeout(r, Duration::from_secs(5))
        });
        assert!(result.is_err(), "reached the server around the proxy");
    }
}
