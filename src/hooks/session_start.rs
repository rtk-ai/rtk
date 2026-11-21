//! SessionStart processor: keep the proxy alive for sessions routed through it.
//!
//! Registered by the `rtk init -g` proxy on-ramp. When the session's
//! `ANTHROPIC_BASE_URL` points at a loopback address RTK was set up for and
//! nothing listens there, launch `rtk llm-proxy start` (a no-op when the
//! daemon is already running) and report through `systemMessage` —
//! user-visible, never model context, so RTK spends no tokens configuring
//! itself. A listener that does not answer as the RTK proxy is never replaced:
//! the hook warns if RTK was set up for its address. An address RTK was not set
//! up for belongs to whoever runs it, listening or not, so the hook leaves it
//! alone: starting RTK on a stopped gateway's port would take the gateway's
//! traffic and keep the gateway from binding. A problem is at most a warning;
//! nothing here disrupts the session.

use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::Context;

pub(super) fn run() {
    if let Some(message) = ensure_proxy() {
        let payload = serde_json::json!({ "systemMessage": message });
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{payload}");
        let _ = out.flush();
    }
}

pub fn ensure_proxy() -> Option<String> {
    let url = crate::core::user_env::var("ANTHROPIC_BASE_URL")?;
    ensure_proxy_impl(
        &url,
        |addr| probe(addr, Duration::from_millis(150)),
        |addr| is_listening(addr, Duration::from_millis(150)),
        rtk_set_up_for,
        spawn_start,
        Duration::from_millis(700),
    )
}

fn ensure_proxy_impl(
    url: &str,
    probe: impl Fn(&str) -> bool,
    is_listening: impl Fn(&str) -> bool,
    set_up_for: impl FnOnce(&[String]) -> bool,
    spawn: impl FnOnce(&str) -> anyhow::Result<()>,
    wait: Duration,
) -> Option<String> {
    let addrs = loopback_addrs(url)?;
    let addr = addrs.first()?;
    if addrs.iter().any(|addr| probe(addr)) {
        return None;
    }
    let listening = addrs.iter().any(|addr| is_listening(addr));
    if !set_up_for(&addrs) {
        return None;
    }
    if listening {
        return Some(format!(
            "rtk: settings.json or RTK_PROXY_BIND expects the RTK proxy at {url}, but whatever holds that port is not answering as the RTK proxy — requests may fail or reach another service; free the port and run `rtk llm-proxy start --bind {addr}`, or remove that setting"
        ));
    }
    match spawn(addr) {
        Ok(()) => {
            let deadline = Instant::now() + wait;
            loop {
                if probe(addr) {
                    return Some(format!(
                        "rtk: started the RTK proxy for this session ({url})"
                    ));
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Some(format!(
                "rtk: launched the RTK proxy for {url} but it is not answering yet — a failed first request succeeds on retry; check with `rtk llm-proxy status`"
            ))
        }
        Err(e) => Some(format!(
            "rtk: ANTHROPIC_BASE_URL points at {url} but the RTK proxy is not running and could not be started ({e}) — run `rtk llm-proxy start`, or remove env.ANTHROPIC_BASE_URL from settings.json"
        )),
    }
}

/// Every address a client of `url` can land on, the one RTK binds first.
/// `localhost` may resolve to either loopback family, so a listener on `[::1]`
/// alone still takes the session's requests.
fn loopback_addrs(url: &str) -> Option<Vec<String>> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let hostport = rest.split('/').next()?;
    let (host, port) = hostport.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    match host {
        "localhost" => Some(vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")]),
        "127.0.0.1" | "[::1]" => Some(vec![format!("{host}:{port}")]),
        _ => None,
    }
}

/// Whether RTK was set up to serve one of `addrs`: the settings value the
/// on-ramp writes, or a daemon moved with `RTK_PROXY_BIND`. `daemon_bind` does
/// not fit here, because its default would claim any gateway on port 8080.
fn rtk_set_up_for(addrs: &[String]) -> bool {
    let bind =
        crate::core::user_env::var("RTK_PROXY_BIND").map(|bind| format!("http://{}", bind.trim()));
    [crate::hooks::init::proxy_env_target(), bind]
        .into_iter()
        .flatten()
        .filter_map(|url| loopback_addrs(&url))
        .flatten()
        .any(|addr| addrs.contains(&addr))
}

fn probe(addr: &str, timeout: Duration) -> bool {
    // Identity, not liveness: a bare TCP connect accepts anything listening
    // on the port (8080 is the most common dev-server port), so a stranger
    // there would read as "the proxy is running". GET /health and confirm it is ours.
    crate::llm_proxy::is_rtk_proxy_at(addr, timeout)
}

fn is_listening(addr: &str, timeout: Duration) -> bool {
    addr.parse()
        .ok()
        .is_some_and(|addr| TcpStream::connect_timeout(&addr, timeout).is_ok())
}

fn spawn_start(addr: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("resolve the rtk binary path")?;
    std::process::Command::new(exe)
        .args(["llm-proxy", "start", "--bind", addr])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn `rtk llm-proxy start`")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use crate::core::user_env;
    use crate::hooks::constants::SETTINGS_JSON;
    use crate::hooks::init::with_claude_dir_override;
    use std::cell::Cell;

    /// Answers one health check with `status`, like a gateway that is not RTK.
    /// The returned listener holds the port until the test drops it.
    fn http_gateway(
        status: &'static str,
    ) -> (
        std::net::TcpListener,
        std::net::SocketAddr,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind gateway");
        let addr = listener.local_addr().expect("gateway address");
        let server = listener.try_clone().expect("clone gateway listener");
        let responder = std::thread::spawn(move || {
            use std::io::Read;

            let (mut stream, _) = server.accept().expect("accept health check");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("bound request read");
            let read = stream.read(&mut [0; 1024]).expect("read health check");
            assert!(read > 0, "health check request must not be empty");
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            )
            .expect("answer health check");
        });
        (listener, addr, responder)
    }

    fn write_base_url(claude_dir: &std::path::Path, url: &str) {
        std::fs::write(
            claude_dir.join(SETTINGS_JSON),
            serde_json::json!({ "env": { "ANTHROPIC_BASE_URL": url } }).to_string(),
        )
        .expect("write settings.json");
    }

    fn addrs_of(url: &str) -> Vec<String> {
        loopback_addrs(url).expect("a loopback url")
    }

    #[test]
    fn only_loopback_urls_are_ours() {
        assert_eq!(addrs_of("http://127.0.0.1:8080"), ["127.0.0.1:8080"]);
        assert_eq!(
            addrs_of("http://localhost:9000/v1"),
            ["127.0.0.1:9000", "[::1]:9000"]
        );
        assert_eq!(addrs_of("https://[::1]:8080"), ["[::1]:8080"]);
        assert_eq!(loopback_addrs("https://api.anthropic.com"), None);
        assert_eq!(loopback_addrs("http://127.0.0.1"), None);
        assert_eq!(loopback_addrs("ftp://127.0.0.1:8080"), None);
    }

    #[test]
    fn remote_url_is_silent() {
        let out = ensure_proxy_impl(
            "https://api.anthropic.com",
            |_| panic!("must not probe a remote url"),
            |_| panic!("must not connect to a remote url"),
            |_| panic!("must not read the setup for a remote url"),
            |_| panic!("must not spawn for a remote url"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn running_daemon_is_silent_and_never_spawned() {
        let out = ensure_proxy_impl(
            "http://127.0.0.1:8080",
            |_| true,
            |_| panic!("the health response already confirms a listener"),
            |_| panic!("a daemon that answers needs no setup check"),
            |_| panic!("must not spawn when the daemon answers"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn occupied_port_is_silent_when_rtk_was_not_set_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind gateway");
        let addr = listener.local_addr().expect("gateway address");
        let out = ensure_proxy_impl(
            &format!("http://{addr}"),
            |addr| probe(addr, Duration::from_millis(150)),
            |addr| is_listening(addr, Duration::from_millis(150)),
            |_| false,
            |_| panic!("must not start a proxy on an occupied port"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn non_rtk_http_server_is_silent_when_rtk_was_not_set_up() {
        for status in ["200 OK", "401 Unauthorized", "404 Not Found"] {
            let (_listener, addr, responder) = http_gateway(status);
            let out = ensure_proxy_impl(
                &format!("http://{addr}"),
                |addr| probe(addr, Duration::from_secs(2)),
                |addr| is_listening(addr, Duration::from_secs(2)),
                |_| false,
                |_| panic!("must not start a proxy over a gateway returning {status}"),
                Duration::ZERO,
            );
            responder.join().expect("gateway responder");
            assert_eq!(out, None, "gateway returning {status}");
        }
    }

    #[test]
    fn hung_daemon_warns_when_settings_name_its_address() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind daemon");
            let addr = listener.local_addr().expect("daemon address");
            let url = format!("http://{addr}");
            write_base_url(claude_dir, &url);
            let out = ensure_proxy_impl(
                &url,
                |addr| probe(addr, Duration::from_millis(150)),
                |addr| is_listening(addr, Duration::from_millis(150)),
                rtk_set_up_for,
                |_| panic!("must not start a proxy on an occupied port"),
                Duration::ZERO,
            );
            let msg = out.expect("a port RTK was set up for must not go quiet");
            assert!(msg.contains(&url), "{msg}");
            assert!(msg.contains("not answering as the RTK proxy"), "{msg}");
            assert!(msg.contains(&format!("--bind {addr}")), "{msg}");
        });
    }

    #[test]
    fn another_service_warns_when_rtk_proxy_bind_names_its_address() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |_| {
            let (_listener, addr, responder) = http_gateway("200 OK");
            let bind = addr.to_string();
            let out = user_env::with_vars(&[("RTK_PROXY_BIND", Some(bind.as_str()))], || {
                ensure_proxy_impl(
                    &format!("http://{addr}"),
                    |addr| probe(addr, Duration::from_secs(2)),
                    |addr| is_listening(addr, Duration::from_secs(2)),
                    rtk_set_up_for,
                    |_| panic!("must not start a proxy over another service"),
                    Duration::ZERO,
                )
            });
            responder.join().expect("gateway responder");
            let msg = out.expect("a port RTK was set up for must not go quiet");
            assert!(msg.contains("not answering as the RTK proxy"), "{msg}");
        });
    }

    #[test]
    fn setup_is_read_from_settings_and_rtk_proxy_bind_only() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            let session = addrs_of("http://127.0.0.1:8080");
            assert!(
                !rtk_set_up_for(&session),
                "the default port alone must not claim a gateway"
            );

            let bound = |bind, addrs: &[String]| {
                user_env::with_vars(&[("RTK_PROXY_BIND", Some(bind))], || rtk_set_up_for(addrs))
            };
            assert!(bound(" 127.0.0.1:8080 ", &session));
            assert!(bound("localhost:8080", &session));
            assert!(!bound("127.0.0.1:8580", &session));
            assert!(!bound("", &session));

            write_base_url(claude_dir, "http://localhost:8080/");
            assert!(rtk_set_up_for(&session));
            assert!(rtk_set_up_for(&addrs_of("http://[::1]:8080")));
            assert!(!rtk_set_up_for(&addrs_of("http://127.0.0.1:8580")));

            write_base_url(claude_dir, "https://api.anthropic.com");
            assert!(!rtk_set_up_for(&session));
        });
    }

    #[test]
    fn localhost_gateway_on_ipv6_only_is_never_replaced() {
        let only_ipv6 = |addr: &str| addr == "[::1]:8580";
        let silent = ensure_proxy_impl(
            "http://localhost:8580/v1",
            |_| false,
            only_ipv6,
            |_| false,
            |_| panic!("must not start a proxy while [::1] takes the requests"),
            Duration::ZERO,
        );
        assert_eq!(silent, None);

        let warned = ensure_proxy_impl(
            "http://localhost:8580/v1",
            |_| false,
            only_ipv6,
            |_| true,
            |_| panic!("must not start a proxy while [::1] takes the requests"),
            Duration::ZERO,
        );
        let msg = warned.expect("a port RTK was set up for must not go quiet");
        assert!(msg.contains("not answering as the RTK proxy"), "{msg}");
    }

    #[test]
    fn localhost_daemon_on_ipv6_only_counts_as_running() {
        let out = ensure_proxy_impl(
            "http://localhost:8580",
            |addr| addr == "[::1]:8580",
            |_| panic!("the health response already confirms a listener"),
            |_| panic!("a daemon that answers needs no setup check"),
            |_| panic!("must not spawn when the daemon answers"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn localhost_sees_a_real_ipv6_listener() {
        let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
            return; // No IPv6 loopback on this machine.
        };
        let port = listener.local_addr().expect("gateway address").port();
        let out = ensure_proxy_impl(
            &format!("http://localhost:{port}"),
            |addr| probe(addr, Duration::from_millis(150)),
            |addr| is_listening(addr, Duration::from_millis(150)),
            |_| false,
            |_| panic!("must not start a proxy while [::1] takes the requests"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn free_port_is_left_alone_when_rtk_was_not_set_up() {
        let out = ensure_proxy_impl(
            "http://127.0.0.1:4000",
            |_| false,
            |_| false,
            |_| false,
            |_| panic!("must not start a proxy on a stopped gateway's port"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
    }

    #[test]
    fn stopped_gateway_is_not_replaced_when_settings_name_another_port() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            write_base_url(claude_dir, "http://127.0.0.1:8080");
            let free = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a port");
            let addr = free.local_addr().expect("free address");
            drop(free);
            let out = user_env::with_vars(&[("RTK_PROXY_BIND", None)], || {
                ensure_proxy_impl(
                    &format!("http://{addr}"),
                    |addr| probe(addr, Duration::from_millis(150)),
                    |addr| is_listening(addr, Duration::from_millis(150)),
                    rtk_set_up_for,
                    |_| panic!("must not start a proxy on a stopped gateway's port"),
                    Duration::ZERO,
                )
            });
            assert_eq!(out, None);
        });
    }

    #[test]
    fn dead_daemon_is_started_where_settings_point() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |claude_dir| {
            write_base_url(claude_dir, "http://127.0.0.1:8080");
            let spawned = Cell::new(false);
            let out = user_env::with_vars(&[("RTK_PROXY_BIND", None)], || {
                ensure_proxy_impl(
                    "http://127.0.0.1:8080",
                    |_| false,
                    |_| false,
                    rtk_set_up_for,
                    |_| {
                        spawned.set(true);
                        Ok(())
                    },
                    Duration::ZERO,
                )
            });
            assert!(spawned.get(), "a dead daemon RTK was set up for is started");
            assert!(
                out.expect("a start is reported")
                    .contains("not answering yet")
            );
        });
    }

    #[test]
    fn dead_daemon_is_started_and_reported() {
        let calls = Cell::new(0usize);
        let out = ensure_proxy_impl(
            "http://127.0.0.1:8080",
            |_| {
                calls.set(calls.get() + 1);
                calls.get() > 1
            },
            |_| false,
            |_| true,
            |_| Ok(()),
            Duration::from_millis(200),
        );
        let msg = out.expect("a start must be reported");
        assert!(msg.contains("started the RTK proxy"), "{msg}");
        assert!(msg.contains("http://127.0.0.1:8080"), "{msg}");
    }

    #[test]
    fn dead_daemon_is_started_on_the_port_the_session_uses() {
        let spawned_on = std::cell::RefCell::new(String::new());
        ensure_proxy_impl(
            "http://localhost:8580/v1",
            |_| false,
            |_| false,
            |_| true,
            |addr| {
                *spawned_on.borrow_mut() = addr.to_string();
                Ok(())
            },
            Duration::ZERO,
        );
        assert_eq!(spawned_on.into_inner(), "127.0.0.1:8580");
    }

    #[test]
    fn slow_bind_is_reported_honestly() {
        let out = ensure_proxy_impl(
            "http://127.0.0.1:8080",
            |_| false,
            |_| false,
            |_| true,
            |_| Ok(()),
            Duration::ZERO,
        );
        let msg = out.expect("a pending start must be reported");
        assert!(msg.contains("not answering yet"), "{msg}");
    }

    #[test]
    fn failed_spawn_names_the_fix() {
        let out = ensure_proxy_impl(
            "http://127.0.0.1:8080",
            |_| false,
            |_| false,
            |_| true,
            |_| anyhow::bail!("binary missing"),
            Duration::ZERO,
        );
        let msg = out.expect("a failed start must be reported");
        assert!(msg.contains("could not be started"), "{msg}");
        assert!(msg.contains("rtk llm-proxy start"), "{msg}");
        assert!(msg.contains("binary missing"), "{msg}");
    }
}

#[cfg(test)]
mod probe_identity {
    use super::*;

    /// The probe must verify the rtk-proxy identity, not just that
    /// something answers TCP on the port — 8080 is the most common dev-server
    /// port, and a bare connect accepts any of them as "the proxy running".
    #[test]
    fn probe_rejects_a_stranger_on_the_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                // Answer like a generic dev server: 200 OK, not rtk-proxy.
                use std::io::Write;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        assert!(
            !probe(&addr, Duration::from_millis(300)),
            "a non-rtk server answering on the port must not count as the proxy"
        );
    }
}
