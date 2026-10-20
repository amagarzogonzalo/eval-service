//! Offline proxy tests: a fake OpenAI-compatible upstream is bound to
//! loopback and the proxy is exercised end to end — key injection,
//! model swap, event emission, budget reservation, SSE passthrough.
//! No network egress anywhere (loopback only).

use eval_proxy::{ProxyConfig, bind, serve_on};
use eval_usage::event::{EventOrigin, Outcome, UsageContext};
use eval_usage::store::UsageStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

static STORE_SEQ: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct Script {
    status: u16,
    content_type: String,
    chunks: Vec<Vec<u8>>,
    chunked: bool,
}

impl Script {
    fn json(body: String) -> Self {
        Self {
            status: 200,
            content_type: "application/json".into(),
            chunks: vec![body.into_bytes()],
            chunked: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Recorded {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

type Seen = Arc<Mutex<Vec<Recorded>>>;

/// Fake upstream: serves one script per incoming connection.
async fn start_upstream(scripts: Vec<Script>) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let queue = Arc::new(Mutex::new(scripts.into_iter().collect::<Vec<_>>()));
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let seen_task = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let queue = queue.clone();
            let seen_task = seen_task.clone();
            tokio::spawn(async move {
                if let Ok(recorded) = read_one(&mut sock).await {
                    let script = queue.lock().unwrap().pop();
                    seen_task.lock().unwrap().push(recorded);
                    if let Some(script) = script {
                        let _ = serve_script(&mut sock, &script).await;
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, seen)
}

async fn read_one(sock: &mut TcpStream) -> std::io::Result<Recorded> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = sock.read(&mut byte).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof",
            ));
        }
        head.push(byte[0]);
        if head.len() > 32 * 1024 || head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head);
    let mut lines = head_text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        sock.read_exact(&mut body).await?;
    }
    Ok(Recorded {
        path: format!("{method} {path}"),
        headers,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    })
}

async fn serve_script(sock: &mut TcpStream, script: &Script) -> std::io::Result<()> {
    if script.chunked {
        let head = format!(
            "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            script.status, script.content_type
        );
        sock.write_all(head.as_bytes()).await?;
        for chunk in &script.chunks {
            sock.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                .await?;
            sock.write_all(chunk).await?;
            sock.write_all(b"\r\n").await?;
        }
        sock.write_all(b"0\r\n\r\n").await?;
    } else {
        let body = script.chunks.concat();
        let head = format!(
            "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            script.status,
            script.content_type,
            body.len()
        );
        sock.write_all(head.as_bytes()).await?;
        sock.write_all(&body).await?;
    }
    sock.flush().await
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let unique = STORE_SEQ.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("eval-proxy-test-{}-{unique}", std::process::id()));
        Self(path)
    }

    fn events(&self, project: &str) -> Vec<eval_usage::event::LlmEvent> {
        UsageStore::new(&self.0)
            .read_events(project)
            .expect("store readable")
            .0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn base_config(
    port: u16,
    upstream: String,
    root: &TestDir,
    project: &str,
    key_env: &'static str,
) -> ProxyConfig {
    ProxyConfig {
        bind_addr: "127.0.0.1".into(),
        port,
        upstream,
        key_env: key_env.into(),
        project: project.into(),
        provider_label: "openai-compat".into(),
        tag: None,
        run_id: None,
        context: UsageContext::Benchmark,
        origin: EventOrigin::Live,
        model_overrides: BTreeMap::new(),
        max_requests: None,
        max_cost_usd: None,
        price_in_per_1m: None,
        price_out_per_1m: None,
        out_reserve_tokens: 1024,
        store_root: root.0.clone(),
        upstream_timeout: Duration::from_secs(10),
        cassette_dir: None,
    }
}

async fn spawn_proxy(config: ProxyConfig) -> u16 {
    let (listener, addr) = bind(&config).await.expect("bind");
    tokio::spawn(serve_on(listener, config));
    addr.port()
}

async fn send(port: u16, body: &Value) -> (u16, String) {
    let payload = body.to_string();
    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    sock.write_all(request.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

async fn send_raw_include_headers(port: u16, body: &Value) -> String {
    let payload = body.to_string();
    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    sock.write_all(request.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    String::from_utf8_lossy(&raw).to_string()
}

fn valid_response(model: &str) -> String {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 3},
            "completion_tokens_details": {"reasoning_tokens": 2}
        }
    })
    .to_string()
}

fn chat_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": "Summarize the digest"}]
    })
}

#[tokio::test]
async fn forwards_injects_key_and_records_reported_usage() {
    let key_a = "EVAL_PROXY_TEST_KEY_A";
    unsafe { std::env::set_var(key_a, "test-key-a") };
    let root = TestDir::new();
    let (upstream_port, seen) = start_upstream(vec![Script::json(valid_response("m1"))]).await;
    let port = spawn_proxy(base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-a",
        key_a,
    ))
    .await;
    let (status, body) = send(port, &chat_body("m1")).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("\"prompt_tokens\":11"),
        "passthrough body: {body}"
    );
    let recorded = seen.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "one upstream call");
    let call = &recorded[0];
    assert_eq!(call.path, "POST /v1/chat/completions");
    let auth = call
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    assert_eq!(auth, "Bearer test-key-a", "real key injected");
    assert_eq!(call.body["model"], "m1");
    let events = root.events("proj-a");
    assert_eq!(events.len(), 1, "one llm/v1 event per attempt");
    let event = &events[0];
    assert_eq!(event.outcome, Outcome::Ok);
    assert_eq!(event.model, "m1");
    assert_eq!(event.input_tokens, Some(11));
    assert_eq!(event.output_tokens, Some(7));
    assert_eq!(event.cached_tokens, Some(3));
    assert_eq!(event.reasoning_tokens, Some(2));
    assert!(!event.streaming);
    assert!(event.duration_ms.is_some());
    assert_eq!(event.context, UsageContext::Benchmark);
    assert_eq!(event.origin, EventOrigin::Live);
    let extensions = event.extensions.clone().unwrap();
    assert_eq!(extensions.get("proxy"), Some(&json!(true)));
}

#[tokio::test]
async fn model_swap_is_applied_and_labeled() {
    let key_b = "EVAL_PROXY_TEST_KEY_B";
    unsafe { std::env::set_var(key_b, "test-key-b") };
    let root = TestDir::new();
    let (upstream_port, seen) = start_upstream(vec![Script::json(valid_response("m2"))]).await;
    let mut config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-b",
        key_b,
    );
    config
        .model_overrides
        .insert("expensive-a".to_string(), "m2".to_string());
    let port = spawn_proxy(config).await;
    let (status, _) = send(port, &chat_body("expensive-a")).await;
    assert_eq!(status, 200);
    let recorded = seen.lock().unwrap().clone();
    assert_eq!(
        recorded[0].body["model"], "m2",
        "upstream receives swapped model"
    );
    let events = root.events("proj-b");
    assert_eq!(events[0].model, "m2");
    assert_eq!(
        events[0]
            .extensions
            .as_ref()
            .unwrap()
            .get("requested_model"),
        Some(&json!("expensive-a"))
    );
}

#[tokio::test]
async fn request_budget_caps_forwarded_attempts() {
    let key_c = "EVAL_PROXY_TEST_KEY_C";
    unsafe { std::env::set_var(key_c, "test-key-c") };
    let root = TestDir::new();
    let (upstream_port, seen) = start_upstream(vec![
        Script::json(valid_response("m1")),
        Script::json(valid_response("m1")),
    ])
    .await;
    let mut config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-c",
        key_c,
    );
    config.max_requests = Some(2);
    let port = spawn_proxy(config).await;
    let body = chat_body("m1");
    let (first, _) = send(port, &body).await;
    let (second, _) = send(port, &body).await;
    let (third, third_body) = send(port, &body).await;
    assert_eq!(first, 200);
    assert_eq!(second, 200);
    assert_eq!(third, 429);
    assert!(third_body.contains("request_budget_exhausted"));
    assert_eq!(seen.lock().unwrap().len(), 2, "third never forwarded");
    assert_eq!(root.events("proj-c").len(), 2);
}

#[tokio::test]
async fn cost_budget_without_prices_fails_closed() {
    let root = TestDir::new();
    let mut config = base_config(
        0,
        "http://127.0.0.1:9".into(),
        &root,
        "proj-d",
        "EVAL_PROXY_TEST_KEY_D",
    );
    config.max_cost_usd = Some(1.0);
    assert!(config.validate().is_err(), "unpriced cost budget refused");
}

#[tokio::test]
async fn cost_budget_reserves_before_forwarding() {
    unsafe { std::env::set_var("EVAL_PROXY_TEST_KEY_E", "test-key-e") };
    let root = TestDir::new();
    // No upstream reachable: a rejected request must never be forwarded.
    let mut config = base_config(
        0,
        "http://127.0.0.1:9".into(),
        &root,
        "proj-e",
        "EVAL_PROXY_TEST_KEY_E",
    );
    // 0.10 USD per 1M input, 0.60 per 1M output, reserve cap 5 cents. A
    // request reserving >5 cents must be rejected pre-forward.
    config.price_in_per_1m = Some(0.10);
    config.price_out_per_1m = Some(0.60);
    config.out_reserve_tokens = 100_000;
    config.max_cost_usd = Some(0.05);
    let port = spawn_proxy(config).await;
    let mut body = chat_body("m1");
    body["messages"] = json!([{"role": "user", "content": "x".repeat(2000)}]);
    let (status, rejected_body) = send(port, &body).await;
    assert_eq!(status, 402);
    assert!(rejected_body.contains("cost_budget_exhausted"));
    assert_eq!(root.events("proj-e").len(), 0, "rejection records no event");
}

#[tokio::test]
async fn cost_budget_priced_and_reconciled() {
    let key_f = "EVAL_PROXY_TEST_KEY_F";
    unsafe { std::env::set_var(key_f, "test-key-f") };
    let root = TestDir::new();
    let (upstream_port, _) = start_upstream(vec![Script::json(valid_response("m1"))]).await;
    let mut config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-f",
        key_f,
    );
    config.price_in_per_1m = Some(0.10);
    config.price_out_per_1m = Some(0.60);
    config.out_reserve_tokens = 500;
    config.max_cost_usd = Some(10.0);
    let port = spawn_proxy(config).await;
    let (status, _) = send(port, &chat_body("m1")).await;
    assert_eq!(status, 200);
    assert_eq!(root.events("proj-f").len(), 1);
}

#[tokio::test]
async fn upstream_error_passes_through_and_records_event() {
    let key_g = "EVAL_PROXY_TEST_KEY_G";
    unsafe { std::env::set_var(key_g, "test-key-g") };
    let root = TestDir::new();
    let (upstream_port, seen) = start_upstream(vec![Script {
        status: 429,
        content_type: "application/json".into(),
        chunks: vec![br#"{"error":{"message":"rate limited"}}"#.to_vec()],
        chunked: false,
    }])
    .await;
    let port = spawn_proxy(base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-g",
        "EVAL_PROXY_TEST_KEY_G",
    ))
    .await;
    let (status, body) = send(port, &chat_body("m1")).await;
    assert_eq!(status, 429);
    assert!(body.contains("rate limited"));
    assert!(!seen.lock().unwrap().is_empty());
    let events = root.events("proj-g");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, Outcome::Error);
    assert_eq!(events[0].error_code.as_deref(), Some("http_429"));
    assert!(events[0].retriable);
    assert_eq!(
        events[0].input_tokens, None,
        "absent stays absent, never zero"
    );
}

#[tokio::test]
async fn sse_streams_through_and_extracts_final_usage() {
    unsafe { std::env::set_var("EVAL_PROXY_TEST_KEY_H", "test-key-h") };
    let root = TestDir::new();
    let chunk1 = br#"data: {"id":"1","choices":[{"delta":{"content":"he"}}]}"#.to_vec();
    let chunk2: Vec<u8> = "data: {\"id\":\"2\",\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n"
        .as_bytes()
        .to_vec();
    let (upstream_port, seen) = start_upstream(vec![Script {
        status: 200,
        content_type: "text/event-stream".into(),
        chunks: vec![chunk1, chunk2],
        chunked: true,
    }])
    .await;
    let config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-h",
        "EVAL_PROXY_TEST_KEY_H",
    );
    let port = spawn_proxy(config).await;
    let mut body = chat_body("m1");
    body["stream"] = json!(true);
    let raw = send_raw_include_headers(port, &body).await;
    assert!(raw.starts_with("HTTP/1.1 200"), "client status line: {raw}");
    assert!(raw.contains("Transfer-Encoding: chunked"));
    assert!(raw.contains("data: {\"id\":\"1\""), "stream relayed: {raw}");
    assert!(raw.contains("data: [DONE]"));
    assert_eq!(seen.lock().unwrap().len(), 1);
    // The proxy added include_usage on the forwarded request.
    let forwarded = &seen.lock().unwrap()[0];
    assert_eq!(
        forwarded.body["stream_options"]["include_usage"],
        json!(true)
    );
    let events = root.events("proj-h");
    assert_eq!(events.len(), 1);
    assert!(events[0].streaming);
    assert!(events[0].ttft_ms.is_some(), "first chunk time observed");
    assert_eq!(events[0].input_tokens, Some(9));
    assert_eq!(events[0].output_tokens, Some(4));
}

#[tokio::test]
async fn rejects_non_json_bad_paths_and_non_post() {
    unsafe { std::env::set_var("EVAL_PROXY_TEST_KEY_I", "test-key-i") };
    let root = TestDir::new();
    let port = spawn_proxy(base_config(
        0,
        "http://127.0.0.1:9".into(),
        &root,
        "proj-i",
        "EVAL_PROXY_TEST_KEY_I",
    ))
    .await;

    let payload = "not json";
    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    sock.write_all(request.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 400"));

    let (status, _) = send_path(port, "/v1/other", &chat_body("m1")).await;
    assert_eq!(status, 404);

    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    sock.write_all(b"GET /v1/chat/completions HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 405"));
}

async fn send_path(port: u16, path: &str, body: &Value) -> (u16, String) {
    let payload = body.to_string();
    let mut sock = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    sock.write_all(request.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn reservation_bound_overestimates_wire_tokens() {
    assert!(
        eval_proxy::estimate_in_tokens(0) > 0,
        "never reserves zero for a real request"
    );
    assert!(eval_proxy::estimate_in_tokens(3000) >= 1000);
}

async fn spawn_cassette(config: eval_proxy::CassetteServeConfig) -> u16 {
    let (listener, addr) = eval_proxy::cassette::bind(&config).await.expect("bind");
    tokio::spawn(eval_proxy::cassette::serve_on(listener, config));
    addr.port()
}

fn cassette_config(dir: PathBuf, root: &TestDir, project: &str) -> eval_proxy::CassetteServeConfig {
    eval_proxy::CassetteServeConfig {
        port: 0,
        dir,
        project: project.into(),
        provider_label: "openai-compat".into(),
        tag: None,
        run_id: None,
        context: UsageContext::Benchmark,
        store_root: root.0.clone(),
    }
}

#[tokio::test]
async fn cassette_records_then_replays_offline_and_miss_fails_loud() {
    let key = "EVAL_PROXY_TEST_KEY_J";
    unsafe { std::env::set_var(key, "test-key-j") };
    let root = TestDir::new();
    let cassette_dir = root.0.join("cassettes");
    let (upstream_port, seen) = start_upstream(vec![Script::json(valid_response("m1"))]).await;
    let mut config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-j",
        key,
    );
    config.cassette_dir = Some(cassette_dir.clone());
    let port = spawn_proxy(config).await;
    let body = chat_body("m1");
    let (status, _) = send(port, &body).await;
    assert_eq!(status, 200);
    assert_eq!(seen.lock().unwrap().len(), 1);
    let recorded = std::fs::read_dir(&cassette_dir).unwrap().count();
    assert_eq!(recorded, 1, "one cassette recorded");

    let replay_port = spawn_cassette(cassette_config(cassette_dir, &root, "proj-j-replay")).await;
    let (status, replay_body) = send(replay_port, &body).await;
    assert_eq!(status, 200);
    assert!(
        replay_body.contains("\"prompt_tokens\":11"),
        "replayed body: {replay_body}"
    );
    let events = root.events("proj-j-replay");
    assert_eq!(events.len(), 1);
    let hit = &events[0];
    assert_eq!(hit.origin, EventOrigin::Replay);
    assert_eq!(hit.outcome, Outcome::Ok);
    assert_eq!(hit.input_tokens, Some(11));
    assert_eq!(hit.output_tokens, Some(7));
    assert_eq!(
        hit.extensions.as_ref().unwrap().get("cassette"),
        Some(&json!(true))
    );

    let (status, miss_body) = send(replay_port, &chat_body("m2")).await;
    assert_eq!(status, 404, "unmatched request fails loudly");
    assert!(miss_body.contains("cassette_miss"));
    let events = root.events("proj-j-replay");
    assert_eq!(events.len(), 2, "miss is recorded, not hidden");
    let miss = events
        .iter()
        .find(|event| event.error_code.as_deref() == Some("cassette_miss"))
        .expect("miss event");
    assert_eq!(miss.origin, EventOrigin::Replay);
    assert_eq!(miss.outcome, Outcome::Error);
}

#[tokio::test]
async fn cassette_replays_recorded_sse_stream() {
    unsafe { std::env::set_var("EVAL_PROXY_TEST_KEY_K", "test-key-k") };
    let root = TestDir::new();
    let cassette_dir = root.0.join("cassettes");
    let chunk1 = br#"data: {"id":"1","choices":[{"delta":{"content":"he"}}]}"#.to_vec();
    let chunk2: Vec<u8> = "data: {\"id\":\"2\",\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\ndata: [DONE]\n\n"
        .as_bytes()
        .to_vec();
    let (upstream_port, _seen) = start_upstream(vec![Script {
        status: 200,
        content_type: "text/event-stream".into(),
        chunks: vec![chunk1, chunk2],
        chunked: true,
    }])
    .await;
    let mut config = base_config(
        0,
        format!("http://127.0.0.1:{upstream_port}/v1"),
        &root,
        "proj-k",
        "EVAL_PROXY_TEST_KEY_K",
    );
    config.cassette_dir = Some(cassette_dir.clone());
    let port = spawn_proxy(config).await;
    let mut body = chat_body("m1");
    body["stream"] = json!(true);
    let raw = send_raw_include_headers(port, &body).await;
    assert!(raw.starts_with("HTTP/1.1 200"));
    assert!(raw.contains("data: [DONE]"));
    assert_eq!(
        std::fs::read_dir(&cassette_dir).unwrap().count(),
        1,
        "streaming cassette recorded"
    );

    let replay_port = spawn_cassette(cassette_config(cassette_dir, &root, "proj-k-replay")).await;
    let (status, replay_body) = send(replay_port, &body).await;
    assert_eq!(status, 200);
    assert!(
        replay_body.contains("data: [DONE]"),
        "replayed stream: {replay_body}"
    );
    let events = root.events("proj-k-replay");
    assert_eq!(events.len(), 1);
    assert!(events[0].streaming);
    assert_eq!(events[0].origin, EventOrigin::Replay);
    assert_eq!(events[0].input_tokens, Some(9));
    assert_eq!(events[0].output_tokens, Some(4));
}

#[test]
fn cassette_first_write_wins_and_divergent_duplicates_fail() {
    let root = TestDir::new();
    let dir = root.0.join("cassettes");
    let first = eval_proxy::cassette::record_exchange(
        &dir,
        "fingerprint-1",
        b"{\"model\":\"m\"}",
        "m",
        "m",
        false,
        "application/json",
        b"{\"id\":\"one\"}",
    )
    .unwrap();
    assert!(first, "first exchange recorded");
    let second = eval_proxy::cassette::record_exchange(
        &dir,
        "fingerprint-1",
        b"{\"model\":\"m\"}",
        "m",
        "m",
        false,
        "application/json",
        b"{\"id\":\"two\"}",
    )
    .unwrap();
    assert!(!second, "identical fingerprint keeps the first response");
    let loaded = eval_proxy::cassette::load_dir(&dir).unwrap();
    assert_eq!(
        loaded.get("fingerprint-1").unwrap().response_body,
        "{\"id\":\"one\"}"
    );

    std::fs::write(
        dir.join("dup.json"),
        r#"{"cassette_version":1,"id":"fingerprint-1","recorded_at_ms":1,"requested_model":"m","served_model":"m","streaming":false,"content_type":"application/json","request_body":"{}","response_body":"{\"id\":\"three\"}"}"#,
    )
    .unwrap();
    let error = eval_proxy::cassette::load_dir(&dir).unwrap_err();
    assert!(error.to_string().contains("divergent"), "error: {error}");
}
