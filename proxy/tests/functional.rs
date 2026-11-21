//! End-to-end feature tests: build the real router against a wiremock upstream
//! and drive requests through it (tower `oneshot`), asserting the observable
//! behaviour — transparent forwarding, CCR compression (the upstream actually
//! receives a smaller body), usage recording + stats.

use http_body_util::BodyExt;
use rtk_proxy::{AppState, config::Config, server::build_router};
use serde_json::{Value, json};
use tower::ServiceExt; // oneshot
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `ccr` arms the one compressing stage; both stores are in-memory so each
/// test gets a fresh, isolated proxy.
fn cfg(upstream: &str, ccr: bool) -> Config {
    Config {
        cache_align: true,
        normalize: true,
        engine_methods: false,
        bind: "127.0.0.1:0".into(),
        anthropic_upstream: upstream.to_string(),
        openai_upstream: upstream.to_string(),
        openai_path: "/v1/chat/completions".to_string(),
        ccr,
        compression: "off".into(),
        pixel_compress: Default::default(),
        relay_borrowing: true,
        db: ":memory:".into(),
        icm_db: ":memory:".into(),
        icm_ttl_secs: 0,
        upstream_timeout_secs: 0,
        upstream_retries: 0,
    }
}

async fn upstream() -> MockServer {
    let s = MockServer::start().await;
    let body = json!({
        "id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-4-5",
        "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":10,"output_tokens":2}
    });
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&s)
        .await;
    s
}

fn big_tool_result_request() -> Vec<u8> {
    let big: String = (0..4000)
        .map(|i| format!("line {i} of command output\n"))
        .collect();
    serde_json::to_vec(&json!({
        "model":"claude-haiku-4-5","max_tokens":5,
        "messages":[{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content": big}
        ]}]
    }))
    .unwrap()
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn openai_path_override_forwards_to_the_configured_endpoint() {
    let up = MockServer::start().await;
    let ok = json!({
        "id":"c","object":"chat.completion","created":1,"model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}
    });
    // Providers like Z.AI host their OpenAI-compatible endpoint outside /v1;
    // only the configured path answers, the historical one 404s.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok))
        .mount(&up)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&up)
        .await;

    let mut c = cfg(&up.uri(), false);
    c.openai_path = "/chat/completions".into();
    let app = build_router(AppState::new(c));
    let resp = app
        .oneshot(
            axum::http::Request::builder().method("POST").uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("authorization", "Bearer dummy")
                .body(axum::body::Body::from(
                    r#"{"model":"m","max_tokens":5,"messages":[
                        {"role":"user","content":"roda"},
                        {"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"Bash","arguments":"{}"}}]},
                        {"role":"tool","tool_call_id":"call_1","content":"build output line"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "the provider's real endpoint answered");

    let recv = up.received_requests().await.unwrap();
    assert_eq!(recv.len(), 1, "exactly one forward, to the configured path");
    assert_eq!(recv[0].url.path(), "/chat/completions");
}

#[tokio::test]
async fn forwards_and_relays_the_upstream_response() {
    let up = upstream().await;
    let app = build_router(AppState::new(cfg(&up.uri(), false)));
    let resp = app
        .oneshot(
            axum::http::Request::builder().method("POST").uri("/v1/messages")
                .header("content-type", "application/json")
                .header("authorization", "Bearer sk-ant-CALLER")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["content"][0]["text"], "ok", "upstream response relayed");

    // The caller's own API key was forwarded upstream.
    let recv = up.received_requests().await.unwrap();
    let auth = recv[0]
        .headers
        .get("authorization")
        .map(|v| v.to_str().unwrap());
    assert_eq!(auth, Some("Bearer sk-ant-CALLER"));
}

#[tokio::test]
async fn retries_transient_upstream_then_succeeds() {
    let up = MockServer::start().await;
    // Higher-priority mock answers 503 for the first two attempts, then is spent.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .with_priority(1)
        .mount(&up)
        .await;
    let ok = json!({
        "id":"msg_1","type":"message","role":"assistant","model":"claude-haiku-4-5",
        "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":10,"output_tokens":2}
    });
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok))
        .mount(&up)
        .await;

    let mut c = cfg(&up.uri(), false);
    c.upstream_retries = 2;
    let app = build_router(AppState::new(c));
    let resp = app
        .oneshot(
            axum::http::Request::builder().method("POST").uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "retried past the transient 503s");
    let j = body_json(resp).await;
    assert_eq!(j["content"][0]["text"], "ok");
    let recv = up.received_requests().await.unwrap();
    assert_eq!(recv.len(), 3, "attempt + 2 retries");
}

#[tokio::test]
async fn settings_get_and_put_roundtrip() {
    let up = upstream().await;
    // Boot with CCR allowed: the roundtrip exercises veto-and-re-enable
    // WITHIN the boot gate. Enabling past a boot that said off is forbidden
    // (see runtime_settings_cannot_enable_ccr_past_the_boot_gate).
    let app = build_router(AppState::new(cfg(&up.uri(), true)));

    let g = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/admin/settings")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(g.status(), 200);
    let j0 = body_json(g).await;
    assert_eq!(j0["ccr"], true, "this fixture boots with ccr allowed");

    let put = |body: &'static str| {
        app.clone().oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/admin/settings")
                .header("content-type", "application/json")
                .header(rtk_proxy::server::CLI_HEADER, "1")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
    };
    let p0 = put(r#"{"ccr":false}"#).await.unwrap();
    assert_eq!(p0.status(), 200);
    assert_eq!(body_json(p0).await["ccr"], false, "a veto always lands");

    let p = put(r#"{"ccr":true}"#).await.unwrap();
    assert_eq!(p.status(), 200);
    let j1 = body_json(p).await;
    assert_eq!(j1["ccr"], true, "re-enabling within the boot gate lands");
    // Unchanged: the upstream is not settable over HTTP.
    assert_eq!(j1["openai_upstream"], up.uri());

    let g2 = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/admin/settings")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let j2 = body_json(g2).await;
    assert_eq!(j2["ccr"], true);
    assert_eq!(j2["openai_upstream"], up.uri());
    assert_eq!(j2["bind"], "127.0.0.1:0");
    assert_eq!(j2["recording"], true);
}

#[tokio::test]
async fn no_route_serves_request_content() {
    let up = upstream().await;
    // None of these routes is served: the daemon must not hand a
    // conversation to anything that asks.
    for uri in [
        "/stats/requests?limit=50",
        "/stats/context",
        "/stats/by-model",
        "/stats/by-day",
    ] {
        let app = build_router(AppState::new(cfg(&up.uri(), false)));
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), 200, "{uri} must not be served");
    }
}

#[tokio::test]
async fn state_changing_routes_refuse_a_request_a_browser_could_forge() {
    let up = upstream().await;
    // A cross-origin POST carrying no custom header and a simple content type
    // needs no CORS preflight, so a visited page can send it. Requiring a
    // custom header makes the request non-simple: the browser must preflight,
    // and with no CORS layer that preflight fails.
    let app = build_router(AppState::new(cfg(&up.uri(), false)));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/admin/shutdown")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "shutdown without the CLI header");

    let app = build_router(AppState::new(cfg(&up.uri(), false)));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/admin/settings")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"ccr":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "settings write without the CLI header");
}

#[tokio::test]
async fn the_upstream_cannot_be_repointed_over_http() {
    let up = upstream().await;
    let app = build_router(AppState::new(cfg(&up.uri(), false)));
    // The whole key-theft path: forward passes `authorization` and `x-api-key`
    // to whatever upstream is configured, so one accepted write sends the next
    // turn's credential to another host.
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/admin/settings")
                .header("content-type", "application/json")
                .header(rtk_proxy::server::CLI_HEADER, "1")
                .body(axum::body::Body::from(
                    r#"{"anthropic_upstream":"http://attacker.example:9999"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_ne!(
        j["anthropic_upstream"], "http://attacker.example:9999",
        "the upstream must not be settable over HTTP"
    );
}

#[tokio::test]
async fn the_daemon_serves_no_admin_or_metrics_surface() {
    let up = upstream().await;
    // A local, CLI-only daemon has no scraper and no dashboard, so it serves
    // no `/metrics`. These namespaces answer 404 rather than being relayed: a
    // fallback that forwards them upstream makes an unserved route reply 200
    // with the provider's answer, which reads as "served".
    for uri in ["/metrics", "/stats/requests", "/admin/nope"] {
        let app = build_router(AppState::new(cfg(&up.uri(), false)));
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{uri} must be gone, not relayed");
    }
}

#[tokio::test]
async fn ccr_shrinks_the_body_sent_upstream() {
    let up = upstream().await;
    let app = build_router(AppState::new(cfg(&up.uri(), true)));
    let sent = big_tool_result_request();
    let sent_len = sent.len();

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(sent))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let recv = up.received_requests().await.unwrap();
    let upstream_body = &recv[0].body;
    assert!(
        upstream_body.len() < sent_len,
        "upstream must receive a SMALLER body: {} -> {}",
        sent_len,
        upstream_body.len()
    );
    let text = String::from_utf8_lossy(upstream_body);
    assert!(
        text.contains(rtk_proxy::pipeline::ccr::CCR_MARKER_TAG),
        "the CCR marker proves the compress step ran"
    );
    assert!(
        text.contains(rtk_proxy::pipeline::ccr::RETRIEVE_TOOL_NAME),
        "the retrieve tool must be injected so the model can get the original back"
    );
}

#[tokio::test]
async fn records_usage_and_serves_stats() {
    let up = upstream().await;
    let state = AppState::new(cfg(&up.uri(), false));
    let app = build_router(state);

    let _ = app
        .clone()
        .oneshot(
            axum::http::Request::builder().method("POST").uri("/v1/messages")
                .header("content-type", "application/json")
                .header("user-agent", "claude-code/1.0")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    // The recording is spawned; give it a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/stats/summary")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let j = body_json(resp).await;
    assert_eq!(j["total_requests"], 1, "the request was recorded");
    // The buffered relay tallies the upstream usage: the mock reports
    // input_tokens=10 / output_tokens=2, so the store records the exact cost
    // the registry prices those at.
    assert_eq!(
        j["tokens_out"], 2,
        "output tokens tallied from the upstream response"
    );
    let expected = rtk_proxy::pricing::cost_usd_cached("claude-haiku-4-5", 10, 0, 0, 2);
    assert!(expected.is_none(), "rtk ships no prices");
    assert_eq!(
        j["cost_usd"].as_f64(),
        Some(0.0),
        "with no configured price the recorded cost is zero, never invented"
    );
}

#[tokio::test]
async fn tallies_usage_from_a_streamed_sse_response() {
    // Anthropic splits usage across message_start (input) and message_delta
    // (output); the relay must tally it as the SSE bytes flow and record the
    // real output tokens once the stream ends.
    let up = MockServer::start().await;
    let sse = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":50,\"output_tokens\":1}}}\n\n\
data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":77}}\n\n\
data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(
            // set_body_raw sets body + content-type atomically (set_body_string
            // would clobber the content-type back to text/plain).
            ResponseTemplate::new(200).set_body_raw(sse.as_bytes(), "text/event-stream"),
        )
        .mount(&up)
        .await;

    let app = build_router(AppState::new(cfg(&up.uri(), false)));

    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder().method("POST").uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":100,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.contains("text/event-stream"),
        "the SSE content-type is relayed transparently"
    );
    // Drain the streamed body so the relay reaches the end of the upstream stream
    // and fires the deferred usage outcome.
    let relayed = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8_lossy(&relayed).contains("message_delta"),
        "SSE relayed verbatim"
    );

    // The record is spawned when the stream ends; give it a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let stats = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/stats/summary")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let j = body_json(stats).await;
    assert_eq!(j["total_requests"], 1, "the streamed request was recorded");
    assert_eq!(
        j["tokens_out"], 77,
        "output tokens tallied from the SSE stream"
    );
}

#[tokio::test]
async fn serves_health() {
    let app = build_router(AppState::new(cfg("http://127.0.0.1:1", false)));

    let health = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(body_json(health).await["edition"], "local");
}

/// A client points its whole base URL at the proxy, so the endpoints we do not
/// intercept must still reach the provider. Returning a 404 (or an HTML body)
/// where the SDK expects JSON is enough to break the client outright.
#[tokio::test]
async fn messages_dialect_relays_and_unknown_destinations_404() {
    let up = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::path("/v1/messages/count_tokens"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"input_tokens": 42})))
        .mount(&up)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&up)
        .await;

    let app = build_router(AppState::new(cfg(&up.uri(), false)));

    let counted = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages/count_tokens")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        counted.status(),
        200,
        "count_tokens must reach the upstream"
    );
    assert_eq!(body_json(counted).await["input_tokens"], 42);

    let models = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/v1/models")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        models.status(),
        404,
        "outside the configured dialects the destination is unknown — refuse, never guess"
    );
    let body = body_json(models).await;
    assert_eq!(body["error"]["type"], "routing_unknown");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("routing destination is unknown"),
        "the 404 must say why: {body}"
    );
}

/// `x-rtk-session` scopes CCR's cache. Dropping it would make every session
/// share the `default` scope, defeating the per-session isolation the module
/// documents.
#[tokio::test]
async fn session_and_project_headers_reach_the_usage_row() {
    let up = upstream().await;
    let app = build_router(AppState::new(cfg(&up.uri(), true)));

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .header("x-rtk-session", "sess-abc")
                .header("x-rtk-project", "my-repo")
                .body(axum::body::Body::from(big_tool_result_request()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The selector headers are proxy-internal and must not leak upstream.
    let recv = up.received_requests().await.unwrap();
    assert!(recv[0].headers.get("x-rtk-session").is_none());
    assert!(recv[0].headers.get("x-rtk-project").is_none());
}

#[tokio::test]
async fn an_openai_clients_key_never_reaches_the_anthropic_upstream() {
    let anthropic = MockServer::start().await;
    let openai = MockServer::start().await;
    for s in [&anthropic, &openai] {
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(s)
            .await;
    }
    let mut c = cfg(&anthropic.uri(), false);
    c.openai_upstream = openai.uri();
    let app = build_router(AppState::new(c));

    for path in ["/v1/responses", "/v1/embeddings", "/v1/models"] {
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(if path == "/v1/models" { "GET" } else { "POST" })
                    .uri(path)
                    .header("authorization", "Bearer sk-proj-LEAKED-TEST-KEY")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), 500, "{path}: relay must not error out");
    }

    let leaked: Vec<String> = anthropic
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.headers.get("authorization").is_some())
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(
        leaked.is_empty(),
        "OpenAI-dialect credentials were forwarded to the ANTHROPIC upstream on: {leaked:?}"
    );
}

/// What the ICM cache file guarantees about its contents.
/// Three properties, asserted as they SHOULD hold; each failure is the repro.
#[tokio::test]
#[cfg(unix)]
async fn icm_cache_file_is_owner_only_and_stores_verbatim() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("icm.db");
    let mem = rtk_proxy::memory::IcmMemory::open(db.to_str().unwrap()).unwrap();

    let secret = "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    mem.put_exact(
        "ccr/default/testid".to_string(),
        format!("tool output containing a credential: {secret} end"),
    )
    .await
    .unwrap();

    let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode & 0o077,
        0,
        "cache holds raw tool output; group/world must have no access (mode {mode:o})"
    );

    // By design, contents stay verbatim -- retrieve must hand
    // back exactly what was stored -- and the protections are the 0600 mode
    // asserted above plus the TTL sweep below. The secret IS on disk; that is
    // the accepted residual, not an oversight.
    let mut bytes = std::fs::read(&db).unwrap();
    for sibling in ["icm.db-wal", "icm.db-shm"] {
        if let Ok(more) = std::fs::read(dir.path().join(sibling)) {
            bytes.extend(more);
        }
    }
    assert!(String::from_utf8_lossy(&bytes).contains(secret));
}

/// Expiry: ccr/% entries past the TTL are swept; anything
/// outside CCR's namespace is never touched.
#[tokio::test]
async fn ccr_cache_entries_expire_and_user_memories_survive() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("icm.db");
    let path = db.to_str().unwrap();
    {
        let mem = rtk_proxy::memory::IcmMemory::open(path).unwrap();
        mem.put_exact("ccr/s1/old".into(), "stale original".into())
            .await
            .unwrap();
        mem.put_exact("ccr/s1/new".into(), "fresh original".into())
            .await
            .unwrap();
        mem.put_exact("user-note".into(), "not CCR's to delete".into())
            .await
            .unwrap();
    }
    {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute(
            "UPDATE memories SET created_at = datetime('now', '-10 days') WHERE topic = 'ccr/s1/old'",
            [],
        )
        .unwrap();
    }
    let swept = rtk_proxy::memory::prune_ccr_cache(path, 7 * 24 * 60 * 60).unwrap();
    assert_eq!(swept, 1, "exactly the expired ccr entry goes");

    let mem = rtk_proxy::memory::IcmMemory::open(path).unwrap();
    assert_eq!(mem.get_exact("ccr/s1/old".into()).await.unwrap(), None);
    assert_eq!(
        mem.get_exact("ccr/s1/new".into()).await.unwrap().as_deref(),
        Some("fresh original")
    );
    assert_eq!(
        mem.get_exact("user-note".into()).await.unwrap().as_deref(),
        Some("not CCR's to delete")
    );
}

/// Without a session header, every conversation shares the
/// "default" namespace: content cached in conversation A is retrievable from
/// conversation B. Distinct conversations must get distinct cache topics.
#[test]
fn headerless_conversations_do_not_share_a_cache_namespace() {
    use rtk_proxy::model::{InterceptedRequest, Provider};
    let convo = |first_msg: &str| {
        let body = json!({
            "model": "claude-haiku-4-5", "max_tokens": 5,
            "messages": [{"role": "user", "content": first_msg}]
        });
        InterceptedRequest::for_pipeline(Provider::Anthropic, "claude-haiku-4-5".to_string(), body)
    };
    let a = rtk_proxy::pipeline::ccr::session_of(&convo("deploy the staging cluster"));
    let b = rtk_proxy::pipeline::ccr::session_of(&convo("write me a haiku"));
    assert_ne!(
        a, b,
        "two unrelated conversations resolved to one cache namespace ({a:?}) — \
         cross-conversation retrieval"
    );
}

/// An upstream failure must not echo the configured
/// upstream URL to the client: the reqwest error string carries the host, and
/// the sanitizer policy already exists (error.rs) — the relay must use it.
#[tokio::test]
async fn upstream_failure_bodies_do_not_name_the_upstream() {
    let dead = "http://127.0.0.1:1";
    let mut c = cfg(dead, false);
    c.upstream_retries = 0;
    let app = build_router(AppState::new(c));

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body =
        String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(
        !body.contains("127.0.0.1:1"),
        "502 body names the configured upstream: {body}"
    );
}

/// A failed request must not record an invented cost: no
/// `usage` in a 429 body, so there is nothing the provider billed. The
/// summary separates failures instead of summing fiction.
#[tokio::test]
async fn failed_requests_record_no_cost_and_are_counted_apart() {
    let up = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_json(
            json!({"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}),
        ))
        .mount(&up)
        .await;
    let app = build_router(AppState::new(cfg(&up.uri(), false)));

    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(big_tool_result_request()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 429, "the failure itself is relayed");

    // Recording is fire-and-forget after the response; poll until the row lands.
    let mut j = json!(null);
    for _ in 0..50 {
        let stats = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/stats/summary")
                    .header("x-rtk-proxy", "1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        j = body_json(stats).await;
        if j["total_requests"].as_i64() == Some(1) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(j["total_requests"].as_i64(), Some(1), "row recorded: {j}");
    assert_eq!(
        j["cost_usd"].as_f64().unwrap(),
        0.0,
        "nothing was billed, so nothing may be recorded as cost: {j}"
    );
    assert_eq!(
        j["failed_requests"].as_i64().unwrap_or(-1),
        1,
        "the failure is visible, separately: {j}"
    );
}

/// Cache tokens are billed tokens. A cache-heavy turn (300
/// fresh / 180k cache-read / 2k cache-creation / 500 out) must record what
/// the provider charges, not just the fresh sliver.
#[tokio::test]
async fn recorded_cost_includes_cache_reads_and_writes() {
    let up = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":"msg_1","type":"message","role":"assistant","model":"claude-opus-4-6",
            "content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":300,"output_tokens":500,
                     "cache_read_input_tokens":180000,"cache_creation_input_tokens":2000}
        })))
        .mount(&up)
        .await;
    let app = build_router(AppState::new(cfg(&up.uri(), false)));
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-opus-4-6","max_tokens":5,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let expected = rtk_proxy::pricing::cost_usd_cached("claude-opus-4-6", 300, 2000, 180_000, 500);
    assert!(expected.is_none(), "rtk ships no prices");

    let mut j = json!(null);
    for _ in 0..50 {
        let stats = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/stats/summary")
                    .header("x-rtk-proxy", "1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        j = body_json(stats).await;
        if j["total_requests"].as_i64() == Some(1) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let recorded = j["cost_usd"].as_f64().unwrap();
    assert_eq!(
        recorded, 0.0,
        "with no configured price, cache-heavy usage still records zero cost"
    );
}

/// The boot gate (pipeline rules / config) can veto CCR;
/// a runtime PUT must not out-rank it. Persisted settings may veto, never
/// enable — the invariant effective_ccr documents.
#[tokio::test]
async fn runtime_settings_cannot_enable_ccr_past_the_boot_gate() {
    let up = upstream().await;
    let state = AppState::new(cfg(&up.uri(), false));
    let flag = state.ccr_enabled.clone();
    let app = build_router(state);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/admin/settings")
                .header("content-type", "application/json")
                .header("x-rtk-proxy", "1")
                .body(axum::body::Body::from(r#"{"ccr": true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        !*flag.read().unwrap(),
        "boot gated CCR off; a runtime PUT enabled it anyway"
    );
}

/// With CCR on, a fresh streamed conversation must still
/// stream: nothing was compressed and no marker exists anywhere, so there is
/// no id a retrieve could name. Forcing every request through the buffered
/// loop would make streaming disappear for the whole session.
#[tokio::test]
async fn ccr_leaves_fresh_streamed_requests_streaming() {
    let up = upstream().await;
    let app = build_router(AppState::new(cfg(&up.uri(), true)));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"model":"claude-haiku-4-5","max_tokens":5,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(resp.status(), 500);

    let sent = &up.received_requests().await.unwrap()[0];
    let body: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(
        body["stream"],
        serde_json::Value::Bool(true),
        "no marker exists anywhere — the upstream must see a streaming request"
    );
}

#[tokio::test]
async fn ccr_first_round_transport_failure_falls_back_to_the_relay() {
    let app = build_router(AppState::new(cfg("http://127.0.0.1:1", true)));
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(big_tool_result_request()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert_ne!(
        status, 500,
        "a transport failure must not surface as a panic: {text}"
    );
    assert!(!text.contains("panicked"), "{text}");
}
