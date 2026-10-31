//! SessionStart processor: keep the proxy alive for sessions routed through it.
//!
//! Registered by the `rtk init -g` proxy on-ramp. When the session's
//! `ANTHROPIC_BASE_URL` points at a loopback address and nothing answers
//! there, launch `rtk llm-proxy start` (a no-op when the daemon is already
//! running) and report through `systemMessage` — user-visible, never model
//! context, so RTK spends no tokens configuring itself. Every failure path
//! returns silence rather than disrupting the session.

use std::io::Write;
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
        spawn_start,
        Duration::from_millis(700),
    )
}

fn ensure_proxy_impl(
    url: &str,
    probe: impl Fn(&str) -> bool,
    spawn: impl FnOnce(&str) -> anyhow::Result<()>,
    wait: Duration,
) -> Option<String> {
    let addr = loopback_hostport(url)?;
    if probe(&addr) {
        return None;
    }
    match spawn(&addr) {
        Ok(()) => {
            let deadline = Instant::now() + wait;
            loop {
                if probe(&addr) {
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

fn loopback_hostport(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let hostport = rest.split('/').next()?;
    let (host, port) = hostport.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    match host {
        "localhost" => Some(format!("127.0.0.1:{port}")),
        "127.0.0.1" | "[::1]" => Some(format!("{host}:{port}")),
        _ => None,
    }
}

fn probe(addr: &str, timeout: Duration) -> bool {
    // Identity, not liveness: a bare TCP connect accepts anything listening
    // on the port (8080 is the most common dev-server port), so a stranger
    // there would read as "the proxy is running". GET /health and confirm it is ours.
    crate::llm_proxy::is_rtk_proxy_at(addr, timeout)
}

fn spawn_start(addr: &str) -> anyhow::Result<()> {
    spawn_start_if_available(addr, crate::llm_proxy::binary_available(), |bind| {
        let exe = std::env::current_exe().context("resolve the rtk binary path")?;
        std::process::Command::new(exe)
            .args(["llm-proxy", "start", "--bind", bind])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawn `rtk llm-proxy start`")?;
        Ok(())
    })
}

/// `Command::new(current_exe()).spawn()` succeeds as long as `rtk` itself
/// exists, even when `rtk-proxy` is missing and the child `rtk llm-proxy start`
/// exits immediately with stderr redirected to null. Check for `rtk-proxy`
/// before spawning so a missing binary surfaces through the `Err` arm instead
/// of waiting 700ms and claiming the proxy was launched.
fn spawn_start_if_available(
    addr: &str,
    binary_available: bool,
    spawn: impl FnOnce(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if !binary_available {
        anyhow::bail!("rtk-proxy is not installed");
    }
    spawn(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn only_loopback_urls_are_ours() {
        assert_eq!(
            loopback_hostport("http://127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            loopback_hostport("http://localhost:9000/v1").as_deref(),
            Some("127.0.0.1:9000")
        );
        assert_eq!(
            loopback_hostport("https://[::1]:8080").as_deref(),
            Some("[::1]:8080")
        );
        assert_eq!(loopback_hostport("https://api.anthropic.com"), None);
        assert_eq!(loopback_hostport("http://127.0.0.1"), None);
        assert_eq!(loopback_hostport("ftp://127.0.0.1:8080"), None);
    }

    #[test]
    fn remote_url_is_silent() {
        let out = ensure_proxy_impl(
            "https://api.anthropic.com",
            |_| panic!("must not probe a remote url"),
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
            |_| panic!("must not spawn when the daemon answers"),
            Duration::ZERO,
        );
        assert_eq!(out, None);
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
            |_| anyhow::bail!("binary missing"),
            Duration::ZERO,
        );
        let msg = out.expect("a failed start must be reported");
        assert!(msg.contains("could not be started"), "{msg}");
        assert!(msg.contains("rtk llm-proxy start"), "{msg}");
        assert!(msg.contains("binary missing"), "{msg}");
    }

    #[test]
    fn missing_binary_reports_could_not_be_started_without_spawning() {
        let out = ensure_proxy_impl(
            "http://127.0.0.1:8080",
            |_| false,
            |addr| {
                spawn_start_if_available(addr, false, |_| {
                    panic!("must not spawn `rtk llm-proxy start` when rtk-proxy is missing")
                })
            },
            Duration::ZERO,
        );
        let msg = out.expect("a missing binary must be reported");
        assert!(msg.contains("could not be started"), "{msg}");
        assert!(msg.contains("rtk-proxy is not installed"), "{msg}");
        assert!(
            msg.contains("remove env.ANTHROPIC_BASE_URL from settings.json"),
            "{msg}"
        );
    }

    #[test]
    fn present_binary_delegates_to_spawn() {
        let spawned_on = std::cell::RefCell::new(String::new());
        spawn_start_if_available("127.0.0.1:8080", true, |addr| {
            *spawned_on.borrow_mut() = addr.to_string();
            Ok(())
        })
        .expect("spawn must succeed when binary is present");
        assert_eq!(spawned_on.into_inner(), "127.0.0.1:8080");
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
