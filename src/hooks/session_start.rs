//! SessionStart processor: keep the proxy alive for sessions routed through it.
//!
//! Registered by the `rtk init -g` proxy on-ramp. When the session's
//! `ANTHROPIC_BASE_URL` points at a loopback address and nothing listens
//! there, launch `rtk llm-proxy start` (a no-op when the daemon is already
//! running) and report through `systemMessage` — user-visible, never model
//! context, so RTK spends no tokens configuring itself. A listener that does
//! not answer as the RTK proxy is never replaced: the hook warns if RTK was set
//! up for its address and otherwise leaves it to whoever runs it. A problem is
//! at most a warning; nothing here disrupts the session.

use std::io::Write;
use std::net::TcpStream;
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
            .filter(|url| rtk_owned(url, crate::hooks::init::CODEX_PROXY_PATHS))
            .map(|url| ("openai_base_url".to_string(), url)),
        "vibe" => crate::hooks::init::vibe_mistral_api_base()
            .filter(|url| rtk_owned(url, &[crate::hooks::init::VIBE_PROXY_PATH]))
            .map(|url| ("providers[mistral].api_base".to_string(), url)),
        _ => None,
    }
}

fn rtk_owned(url: &str, paths: &[&str]) -> bool {
    crate::llm_proxy::is_rtk_url(url, crate::llm_proxy::DEFAULT_BIND, paths)
        || crate::llm_proxy::is_rtk_url(url, &crate::llm_proxy::daemon_bind(), paths)
}

pub(crate) fn routed_through_rtk(agent: &str) -> bool {
    base_url(agent).is_some_and(|(_, url)| loopback_addrs(&url).is_some())
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
    let _ = std::fs::File::create(stamp).and_then(|file| file.set_modified(SystemTime::now()));
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
        |addr| is_listening(addr, Duration::from_millis(150)),
        |addrs| who_set_up(var, addrs),
        spawn_start,
        Duration::from_millis(700),
    )
}

fn ensure_proxy_impl(
    var: &str,
    url: &str,
    probe: impl Fn(&str) -> bool,
    is_listening: impl Fn(&str) -> bool,
    set_up_by: impl FnOnce(&[String]) -> Option<String>,
    spawn: impl FnOnce(&str) -> anyhow::Result<()>,
    wait: Duration,
) -> Option<String> {
    let addrs = loopback_addrs(url)?;
    let addr = addrs.first()?;
    if addrs.iter().any(|addr| probe(addr)) {
        return None;
    }
    if addrs.iter().any(|addr| is_listening(addr)) {
        return set_up_by(&addrs).map(|who| {
            format!(
                "rtk: {who} expects the RTK proxy at {url}, but whatever holds that port is not answering as the RTK proxy — requests may fail or reach another service; free the port and run `rtk llm-proxy start --bind {addr}`, or remove that setting"
            )
        });
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
            "rtk: {var} points at {url} but the RTK proxy is not running and could not be started ({e}) — run `rtk llm-proxy start`, or remove {var} from the agent's settings"
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
fn who_set_up(var: &str, addrs: &[String]) -> Option<String> {
    match var {
        "ANTHROPIC_BASE_URL" => {
            rtk_set_up_for(addrs).then(|| "settings.json or RTK_PROXY_BIND".to_string())
        }
        setting => Some(setting.to_string()),
    }
}

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
pub(crate) fn fake_rtk_daemon() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut request = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut request);
            let body = r#"{"service":"rtk-proxy"}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_isolation;
    use crate::core::user_env;
    use crate::hooks::constants::SETTINGS_JSON;
    use crate::hooks::init::with_claude_dir_override;
    use std::cell::Cell;

    const VAR: &str = "ANTHROPIC_BASE_URL";

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
            VAR,
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
            VAR,
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
            VAR,
            &format!("http://{addr}"),
            |addr| probe(addr, Duration::from_millis(150)),
            |addr| is_listening(addr, Duration::from_millis(150)),
            |_| None,
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
                VAR,
                &format!("http://{addr}"),
                |addr| probe(addr, Duration::from_secs(2)),
                |addr| is_listening(addr, Duration::from_secs(2)),
                |_| None,
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
                VAR,
                &url,
                |addr| probe(addr, Duration::from_millis(150)),
                |addr| is_listening(addr, Duration::from_millis(150)),
                |addrs| who_set_up(VAR, addrs),
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
                    VAR,
                    &format!("http://{addr}"),
                    |addr| probe(addr, Duration::from_secs(2)),
                    |addr| is_listening(addr, Duration::from_secs(2)),
                    |addrs| who_set_up(VAR, addrs),
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
    fn an_agent_setting_rtk_wrote_warns_when_another_listener_holds_its_port() {
        let (_listener, addr, responder) = http_gateway("200 OK");
        let out = ensure_proxy_impl(
            "openai_base_url",
            &format!("http://{addr}/openai/v1"),
            |addr| probe(addr, Duration::from_secs(2)),
            |addr| is_listening(addr, Duration::from_secs(2)),
            |addrs| who_set_up("openai_base_url", addrs),
            |_| panic!("must not start a proxy over another service"),
            Duration::ZERO,
        );
        responder.join().expect("gateway responder");
        let msg = out.expect("a setting RTK wrote must not go quiet");
        assert!(
            msg.starts_with("rtk: openai_base_url expects the RTK proxy at"),
            "{msg}"
        );
    }

    #[test]
    fn claude_code_names_its_setup_and_other_agents_name_their_setting() {
        let tmp = test_isolation::tempdir();
        with_claude_dir_override(&tmp, |_| {
            let session = addrs_of("http://127.0.0.1:8080");
            assert_eq!(who_set_up(VAR, &session), None);
            assert_eq!(
                who_set_up("openai_base_url", &session).as_deref(),
                Some("openai_base_url")
            );
            assert_eq!(
                who_set_up("providers[mistral].api_base", &session).as_deref(),
                Some("providers[mistral].api_base")
            );
            let claimed =
                user_env::with_vars(&[("RTK_PROXY_BIND", Some("127.0.0.1:8080"))], || {
                    who_set_up(VAR, &session)
                });
            assert_eq!(claimed.as_deref(), Some("settings.json or RTK_PROXY_BIND"));
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
            VAR,
            "http://localhost:8580/v1",
            |_| false,
            only_ipv6,
            |_| None,
            |_| panic!("must not start a proxy while [::1] takes the requests"),
            Duration::ZERO,
        );
        assert_eq!(silent, None);

        let warned = ensure_proxy_impl(
            VAR,
            "http://localhost:8580/v1",
            |_| false,
            only_ipv6,
            |_| Some("settings.json or RTK_PROXY_BIND".to_string()),
            |_| panic!("must not start a proxy while [::1] takes the requests"),
            Duration::ZERO,
        );
        let msg = warned.expect("a port RTK was set up for must not go quiet");
        assert!(msg.contains("not answering as the RTK proxy"), "{msg}");
    }

    #[test]
    fn localhost_daemon_on_ipv6_only_counts_as_running() {
        let out = ensure_proxy_impl(
            VAR,
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
            VAR,
            &format!("http://localhost:{port}"),
            |addr| probe(addr, Duration::from_millis(150)),
            |addr| is_listening(addr, Duration::from_millis(150)),
            |_| None,
            |_| panic!("must not start a proxy while [::1] takes the requests"),
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
            |_| false,
            |_| panic!("a free port needs no setup check"),
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
            |_| false,
            |_| panic!("a free port needs no setup check"),
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
            |_| false,
            |_| panic!("a free port needs no setup check"),
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
            |_| false,
            |_| panic!("a free port needs no setup check"),
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
            |_| false,
            |_| panic!("a free port needs no setup check"),
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

    fn write_agent_config(home: &Path, relative: &str, text: &str) {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
        std::fs::write(path, text).expect("write config");
    }

    fn set_age(path: &Path, age: Duration) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open stamp")
            .set_modified(SystemTime::now() - age)
            .expect("set mtime");
    }

    #[test]
    fn vibe_base_url_is_the_mistral_api_base_when_rtk_owns_it() {
        let home = crate::core::test_isolation::tempdir();
        write_agent_config(
            home.path(),
            ".vibe/config.toml",
            "[[providers]]\nname = \"mistral\"\napi_base = \"http://127.0.0.1:8080/mistral/v1\"\n",
        );
        crate::core::test_isolation::with_root(home.path(), || {
            crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", None)], || {
                assert_eq!(
                    base_url("vibe"),
                    Some((
                        "providers[mistral].api_base".to_string(),
                        "http://127.0.0.1:8080/mistral/v1".to_string()
                    ))
                );
            });
        });
    }

    #[test]
    fn vibe_base_url_ignores_a_foreign_api_base() {
        let home = crate::core::test_isolation::tempdir();
        write_agent_config(
            home.path(),
            ".vibe/config.toml",
            "[[providers]]\nname = \"mistral\"\napi_base = \"https://api.mistral.ai/v1\"\n",
        );
        crate::core::test_isolation::with_root(home.path(), || {
            assert_eq!(base_url("vibe"), None);
        });
    }

    #[test]
    fn a_users_own_loopback_server_is_not_routed_through_rtk() {
        for own in [
            "http://127.0.0.1:8080/v1",
            "http://127.0.0.1:8080",
            "http://127.0.0.1:4000/v1",
            "http://localhost:4000/openai/v1",
        ] {
            let home = crate::core::test_isolation::tempdir();
            write_agent_config(
                home.path(),
                ".codex/config.toml",
                &format!("openai_base_url = \"{own}\"\n"),
            );
            write_agent_config(
                home.path(),
                ".vibe/config.toml",
                &format!("[[providers]]\nname = \"mistral\"\napi_base = \"{own}\"\n"),
            );
            crate::core::test_isolation::with_root(home.path(), || {
                crate::core::user_env::with_vars(
                    &[("CODEX_HOME", None), ("RTK_PROXY_BIND", None)],
                    || {
                        for agent in ["codex", "vibe"] {
                            assert_eq!(base_url(agent), None, "{agent}: {own}");
                            assert!(!routed_through_rtk(agent), "{agent}: {own}");
                            assert_eq!(
                                throttled_keepalive(agent, Duration::ZERO),
                                None,
                                "{agent}: {own}"
                            );
                        }
                    },
                );
            });
        }
    }

    #[test]
    fn claude_code_is_routed_by_rtks_url_not_a_remote_or_unset_one() {
        for (url, routed) in [
            (Some("http://127.0.0.1:8080"), true),
            (Some("https://gw.corp"), false),
            (None, false),
        ] {
            crate::core::user_env::with_vars(&[(VAR, url)], || {
                assert_eq!(routed_through_rtk("claude-code"), routed, "{url:?}");
            });
        }
    }

    #[test]
    fn the_default_port_is_rtks_even_when_the_daemon_listens_elsewhere() {
        let paths = crate::hooks::init::CODEX_PROXY_PATHS;
        crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", Some("127.0.0.1:9191"))], || {
            assert!(rtk_owned("http://127.0.0.1:8080/openai/v1", paths));
            assert!(rtk_owned("http://127.0.0.1:9191/openai/v1", paths));
            assert!(!rtk_owned("http://127.0.0.1:7000/openai/v1", paths));
        });
    }

    #[test]
    fn codex_base_url_comes_from_the_config_key_not_the_environment() {
        let home = crate::core::test_isolation::tempdir();
        write_agent_config(
            home.path(),
            ".codex/config.toml",
            "openai_base_url = \"http://127.0.0.1:8080/openai/v1\"\n",
        );
        crate::core::test_isolation::with_root(home.path(), || {
            crate::core::user_env::with_vars(
                &[
                    ("CODEX_HOME", None),
                    ("RTK_PROXY_BIND", None),
                    (
                        "OPENAI_BASE_URL",
                        Some("http://127.0.0.1:8080/chatgpt/backend-api/codex"),
                    ),
                ],
                || {
                    assert_eq!(
                        base_url("codex"),
                        Some((
                            "openai_base_url".to_string(),
                            "http://127.0.0.1:8080/openai/v1".to_string()
                        ))
                    );
                },
            );
        });
    }

    #[test]
    fn codex_base_url_is_none_without_the_config_key() {
        let home = crate::core::test_isolation::tempdir();
        crate::core::test_isolation::with_root(home.path(), || {
            crate::core::user_env::with_vars(
                &[
                    ("CODEX_HOME", None),
                    ("RTK_PROXY_BIND", None),
                    ("OPENAI_BASE_URL", Some("http://127.0.0.1:8080/openai/v1")),
                ],
                || assert_eq!(base_url("codex"), None),
            );
        });
    }

    #[test]
    fn the_throttle_boundary_is_inclusive() {
        let dir = crate::core::test_isolation::tempdir();
        let stamp = dir.path().join("keepalive.vibe");
        touch(&stamp);
        let modified = std::fs::metadata(&stamp).unwrap().modified().unwrap();
        let interval = Duration::from_secs(300);
        assert!(stamp_due(&stamp, modified + interval, interval));
        assert!(!stamp_due(
            &stamp,
            modified + interval - Duration::from_secs(1),
            interval
        ));
    }

    #[test]
    fn a_stamp_from_the_future_is_due() {
        let dir = crate::core::test_isolation::tempdir();
        let stamp = dir.path().join("keepalive.vibe");
        touch(&stamp);
        let modified = std::fs::metadata(&stamp).unwrap().modified().unwrap();
        assert!(stamp_due(
            &stamp,
            modified - Duration::from_secs(3600),
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn throttled_keepalive_stamps_when_due_and_stays_quiet_until_the_interval_passes() {
        let home = crate::core::test_isolation::tempdir();
        let daemon = fake_rtk_daemon();
        let url = format!("http://{daemon}/mistral/v1");
        write_agent_config(
            home.path(),
            ".vibe/config.toml",
            &format!("[[providers]]\nname = \"mistral\"\napi_base = \"{url}\"\n"),
        );
        crate::core::test_isolation::with_root(home.path(), || {
            crate::core::user_env::with_vars(&[("RTK_PROXY_BIND", Some(daemon.as_str()))], || {
                let stamp = crate::core::user_dirs::config()
                    .unwrap()
                    .join("llm-proxy")
                    .join("keepalive.vibe");
                let interval = Duration::from_secs(3600);
                assert!(!stamp.exists());

                assert_eq!(throttled_keepalive("vibe", interval), None);
                assert!(stamp.is_file(), "a due keep-alive must leave a stamp");

                set_age(&stamp, Duration::from_secs(60));
                let before = std::fs::metadata(&stamp).unwrap().modified().unwrap();
                assert_eq!(throttled_keepalive("vibe", interval), None);
                let after = std::fs::metadata(&stamp).unwrap().modified().unwrap();
                assert_eq!(before, after, "a fresh stamp must not be rewritten");

                set_age(&stamp, Duration::from_secs(7200));
                let before = std::fs::metadata(&stamp).unwrap().modified().unwrap();
                assert_eq!(throttled_keepalive("vibe", interval), None);
                let after = std::fs::metadata(&stamp).unwrap().modified().unwrap();
                assert!(after > before, "an expired stamp must be renewed");
            });
        });
    }

    #[test]
    fn throttled_keepalive_does_nothing_for_an_agent_not_routed_through_rtk() {
        let home = crate::core::test_isolation::tempdir();
        crate::core::test_isolation::with_root(home.path(), || {
            assert_eq!(throttled_keepalive("vibe", Duration::from_secs(300)), None);
            let stamp = crate::core::user_dirs::config()
                .unwrap()
                .join("llm-proxy")
                .join("keepalive.vibe");
            assert!(!stamp.exists());
        });
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
