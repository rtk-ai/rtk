//! SessionStart processor: keep the proxy alive for sessions routed through it.
//!
//! Registered by the `rtk init -g` proxy on-ramp. When the session's
//! `ANTHROPIC_BASE_URL` points at a loopback address and nothing answers
//! there, launch `rtk llm-proxy start` (a no-op when the daemon is already
//! running) and report through `systemMessage` — user-visible, never model
//! context, so RTK spends no tokens configuring itself. Every failure path
//! returns silence rather than disrupting the session.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use rtk_engine::harness::{Channel, HarnessProfile};
use serde_json::{Value, json};

pub(super) fn run() {
    if let Some(payload) = response_for("claude-code") {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{payload}");
        let _ = out.flush();
    }
}

pub(crate) fn response_for(agent: &str) -> Option<String> {
    let profile = rtk_engine::harness::profile(agent)?;
    let keepalive = base_url(agent).and_then(|(setting, url)| ensure_proxy_at(&setting, &url));
    if let Some(message) = keepalive.as_deref().filter(|_| agent != "claude-code") {
        let _ = writeln!(std::io::stderr(), "{message}");
    }
    let context = crate::core::pipeline_config::session_start_text(
        &crate::core::pipeline_config::load().resolved,
    );
    encode(profile, keepalive.as_deref(), context.as_deref()).map(|v| v.to_string())
}

fn base_url(agent: &str) -> Option<(String, String)> {
    match agent {
        "claude-code" => crate::core::user_env::var("ANTHROPIC_BASE_URL")
            .map(|url| ("ANTHROPIC_BASE_URL".to_string(), url)),
        "codex" => crate::hooks::init::codex_openai_base_url()
            .map(|url| ("openai_base_url".to_string(), url)),
        "vibe" => crate::hooks::init::vibe_mistral_api_base()
            .map(|url| ("providers[mistral].api_base".to_string(), url)),
        _ => None,
    }
}

pub(crate) fn routed_through_rtk(agent: &str) -> bool {
    base_url(agent).is_some_and(|(_, url)| loopback_hostport(&url).is_some())
}

pub(crate) fn throttled_keepalive(agent: &str, interval: Duration) -> Option<String> {
    let (setting, url) = base_url(agent)?;
    let stamp = crate::core::user_dirs::config()?
        .join("llm-proxy")
        .join(format!("keepalive.{agent}"));
    if !stamp_due(&stamp, SystemTime::now(), interval) {
        return None;
    }
    touch(&stamp);
    ensure_proxy_at(&setting, &url)
}

fn stamp_due(stamp: &Path, now: SystemTime, interval: Duration) -> bool {
    match std::fs::metadata(stamp).and_then(|m| m.modified()) {
        Ok(modified) => now
            .duration_since(modified)
            .map(|age| age >= interval)
            .unwrap_or(true),
        Err(_) => true,
    }
}

fn touch(stamp: &Path) {
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(stamp, b"");
}

pub(crate) fn encode(
    profile: &HarnessProfile,
    keepalive: Option<&str>,
    context: Option<&str>,
) -> Option<Value> {
    let context = context.filter(|_| profile.directive_channels.contains(&Channel::SessionStart));
    match (profile.id, keepalive, context) {
        (_, None, None) => None,
        ("claude-code", message, context) => {
            let mut payload = json!({});
            if let Some(message) = message {
                payload["systemMessage"] = json!(message);
            }
            if let Some(context) = context {
                payload["hookSpecificOutput"] = json!({
                    "hookEventName": crate::hooks::constants::SESSION_START_KEY,
                    "additionalContext": context,
                });
            }
            Some(payload)
        }
        ("codex", keepalive, context) => {
            let text = [keepalive, context]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n\n");
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": crate::hooks::constants::SESSION_START_KEY,
                    "additionalContext": text,
                }
            }))
        }
        ("cursor", _, Some(context)) => Some(json!({ "additional_context": context })),
        ("antigravity", _, Some(context)) => Some(json!({
            "injectSteps": [{ "systemMessage": context }]
        })),
        _ => None,
    }
}

fn ensure_proxy_at(var: &str, url: &str) -> Option<String> {
    ensure_proxy_impl(
        var,
        url,
        |addr| probe(addr, Duration::from_millis(150)),
        spawn_start,
        Duration::from_millis(700),
    )
}

fn ensure_proxy_impl(
    var: &str,
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
            "rtk: {var} points at {url} but the RTK proxy is not running and could not be started ({e}) — run `rtk llm-proxy start`, or remove {var} from the agent's settings"
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
    use std::cell::Cell;

    const VAR: &str = "ANTHROPIC_BASE_URL";

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
            VAR,
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
            VAR,
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
            VAR,
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
            VAR,
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
            VAR,
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
            VAR,
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
    fn spawn_failure_names_the_setting() {
        let out = ensure_proxy_impl(
            "openai_base_url",
            "http://127.0.0.1:8080",
            |_| false,
            |_| Err(anyhow::anyhow!("no binary")),
            Duration::ZERO,
        );
        let message = out.unwrap();
        assert!(message.contains("openai_base_url points at"));
        assert!(message.contains("no binary"));
    }

    #[test]
    fn the_throttle_fires_on_a_missing_or_old_stamp_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp = dir.path().join("llm-proxy").join("keepalive.vibe");
        let five_minutes = Duration::from_secs(300);
        assert!(stamp_due(&stamp, SystemTime::now(), five_minutes));
        touch(&stamp);
        assert!(stamp.is_file());
        assert!(!stamp_due(&stamp, SystemTime::now(), five_minutes));
        assert!(stamp_due(&stamp, SystemTime::now(), Duration::ZERO));
        assert!(stamp_due(
            &stamp,
            SystemTime::now() + Duration::from_secs(600),
            five_minutes
        ));
    }

    #[test]
    fn each_agent_receives_session_context_in_its_own_shape() {
        let profile = |id: &str| rtk_engine::harness::profile(id).unwrap();
        let claude = encode(profile("claude-code"), Some("started"), Some("be terse")).unwrap();
        assert_eq!(claude["systemMessage"], "started");
        assert_eq!(
            claude["hookSpecificOutput"]["additionalContext"],
            "be terse"
        );
        assert_eq!(
            encode(profile("claude-code"), Some("started"), None).unwrap(),
            json!({ "systemMessage": "started" })
        );

        let codex = encode(profile("codex"), None, Some("be terse")).unwrap();
        assert_eq!(codex["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(codex["hookSpecificOutput"]["additionalContext"], "be terse");
        let codex = encode(profile("codex"), Some("started"), None).unwrap();
        assert_eq!(codex["hookSpecificOutput"]["additionalContext"], "started");
        let codex = encode(profile("codex"), Some("started"), Some("be terse")).unwrap();
        assert_eq!(
            codex["hookSpecificOutput"]["additionalContext"],
            "started\n\nbe terse"
        );

        assert_eq!(
            encode(profile("cursor"), None, Some("be terse")).unwrap(),
            json!({ "additional_context": "be terse" })
        );
        assert_eq!(
            encode(profile("antigravity"), None, Some("be terse")).unwrap(),
            json!({ "injectSteps": [{ "systemMessage": "be terse" }] })
        );
        assert_eq!(encode(profile("vibe"), None, Some("be terse")), None);
        assert_eq!(encode(profile("cursor"), None, None), None);
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
