//! Real-binary smoke test: spawn the **compiled** `rtk-proxy` with env-only
//! config and a wiremock upstream, then drive it over real HTTP. This covers the
//! boot path the in-process router tests (`functional.rs`) bypass — `main.rs`
//! env→config parsing, store open, and the real TCP listener — end to end.

use std::process::{Child, Command, Stdio};
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Kills the spawned proxy when the test ends (pass or panic).
struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Grab a free TCP port by binding :0 and releasing it (tiny race, fine for CI).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Poll `/health` until the freshly-spawned binary answers, or fail.
async fn wait_healthy(client: &reqwest::Client, base: &str) {
    for _ in 0..80 {
        if let Ok(r) = client.get(format!("{base}/health")).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("rtk-proxy did not become healthy in time");
}

#[tokio::test]
async fn real_binary_boots_forwards_and_records() {
    let up = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-4-5",
            "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":10,"output_tokens":7}
        })))
        .mount(&up)
        .await;

    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let child = Command::new(env!("CARGO_BIN_EXE_rtk-proxy"))
        .arg("start")
        .env("RTK_PROXY_BIND", format!("127.0.0.1:{port}"))
        .env("RTK_PROXY_ANTHROPIC_UPSTREAM", up.uri())
        .env("RTK_PROXY_OPENAI_UPSTREAM", up.uri())
        .env("RTK_PROXY_CCR", "true")
        .env("RTK_PROXY_DB", ":memory:")
        // Both stores in memory: the ICM default is cwd-relative, so leaving
        // it unset drops a real SQLite file into the crate root on every run.
        .env("RTK_PROXY_ICM_DB", ":memory:")
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn rtk-proxy binary");
    let _guard = Kill(child);

    let client = reqwest::Client::new();
    wait_healthy(&client, &base).await;

    // 1) /health reports the local edition.
    let h: serde_json::Value = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h["edition"], "local", "health edition");
    assert_eq!(
        h["service"], "rtk-proxy",
        "the identity the rtk CLI verifies before trusting the port"
    );

    // 2) A forward relays the upstream response, caller key passed through.
    let r = client
        .post(format!("{base}/v1/messages"))
        .header("content-type", "application/json")
        .header("x-api-key", "sk-ant-CALLER")
        .body(r#"{"model":"claude-haiku-4-5","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "forward status");
    let j: serde_json::Value = r.json().await.unwrap();
    assert_eq!(j["content"][0]["text"], "ok", "relayed upstream body");

    let recv = up.received_requests().await.unwrap();
    assert!(
        recv.iter().any(
            |req| req.headers.get("x-api-key").and_then(|v| v.to_str().ok())
                == Some("sk-ant-CALLER")
        ),
        "the caller's own key reached the upstream"
    );

    // 3) Usage was recorded with the real output-token count.
    tokio::time::sleep(Duration::from_millis(500)).await; // record is spawned
    let s: serde_json::Value = client
        .get(format!("{base}/stats/summary"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(s["total_requests"], 1, "request recorded");
    assert_eq!(s["tokens_out"], 7, "real output tokens tallied");

    // 4) No embedded dashboard: an unmatched path is a plain 404.
    let d = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(d.status(), 404, "no dashboard fallback");
}

/// Run `rtk-proxy <args...>` as a real child process and return its stdout.
fn run_cli(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rtk-proxy"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run rtk-proxy {args:?}: {e}"));
    assert!(
        out.status.success(),
        "rtk-proxy {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
async fn cli_subcommands_talk_to_a_running_daemon_then_stop_it() {
    let port = free_port();
    let bind = format!("127.0.0.1:{port}");
    let base = format!("http://127.0.0.1:{port}");
    // The daemon's transforms come from the active pipeline: without
    // proxy.request rules it forwards unchanged, so give it some. This test
    // drives the CCR runtime toggle, and `rtk:proxy` deliberately does NOT
    // enable CCR, so name the method explicitly.
    let pipeline_path = std::env::temp_dir().join(format!("rtk-proxy-smoke-{port}.toml"));
    std::fs::write(
        &pipeline_path,
        "[pipeline]\nextends = \"rtk:proxy\"\n\
         [[proxy.request.tool_result]]\nname = \"ccr\"\n\
         when = { tool_not_in = [\"read\"], tokens_gt = 1500 }\nuse = [\"ccr\"]\n",
    )
    .expect("write smoke pipeline");
    let child = Command::new(env!("CARGO_BIN_EXE_rtk-proxy"))
        .arg("start")
        .env("RTK_PROXY_BIND", &bind)
        .env("RTK_PROXY_DB", ":memory:")
        .env("RTK_PROXY_ICM_DB", ":memory:")
        .env("RTK_PROXY_PIPELINE", &pipeline_path)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn rtk-proxy binary");
    let _guard = Kill(child);

    let client = reqwest::Client::new();
    wait_healthy(&client, &base).await;

    let status_out = run_cli(&["status", "--bind", &bind]);
    assert!(
        status_out.contains("running"),
        "status should report running, got: {status_out}"
    );

    let stats_out = run_cli(&["stats", "--bind", &bind]);
    assert!(
        stats_out.contains("total requests : 0"),
        "stats should show zero requests, got: {stats_out}"
    );

    // `config get` (no key) returns the full settings JSON. CCR is on unless
    // turned off, and RTK_PROXY_CCR was left unset for this daemon.
    let cfg_out = run_cli(&["config", "get", "--bind", &bind]);
    let cfg_json: serde_json::Value = serde_json::from_str(&cfg_out)
        .unwrap_or_else(|e| panic!("config get output wasn't JSON ({e}): {cfg_out}"));
    assert_eq!(cfg_json["ccr"], true, "ccr defaults on");

    run_cli(&["config", "set", "--bind", &bind, "ccr", "false"]);
    assert_eq!(
        run_cli(&["config", "get", "--bind", &bind, "ccr"]).trim(),
        "false"
    );
    run_cli(&["config", "set", "--bind", &bind, "ccr", "true"]);
    assert_eq!(
        run_cli(&["config", "get", "--bind", &bind, "ccr"]).trim(),
        "true"
    );

    // `config set` on an unrecognized key is rejected client-side, not silently
    // dropped by the server.
    let bad = Command::new(env!("CARGO_BIN_EXE_rtk-proxy"))
        .args(["config", "set", "--bind", &bind, "not_a_real_key", "x"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "unknown setting key should fail");

    let stop_out = run_cli(&["stop", "--bind", &bind]);
    assert!(stop_out.contains("stopped"), "got: {stop_out}");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = client.get(format!("{base}/health")).send().await;
    assert!(
        after.is_err(),
        "daemon should no longer be reachable after `stop`"
    );
}
