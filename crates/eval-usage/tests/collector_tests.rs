//! Collector tests: loopback HTTP inlet over real sockets.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-collect-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn event(id: &str) -> serde_json::Value {
    serde_json::json!({
        "event": "llm.call",
        "event_version": 1,
        "event_id": id,
        "project": "p",
        "request_id": "r1",
        "provider": "fixture",
        "model": "summarizer-a",
        "started_at_ms": 1700000000000u64,
        "duration_ms": 50,
        "input_tokens": 100,
        "output_tokens": 20,
        "outcome": "ok",
        "context": "benchmark",
    })
}

async fn spawn(token_file: Option<PathBuf>) -> (PathBuf, u16, tokio::task::JoinHandle<()>) {
    let dir = tmpdir("srv");
    // Ephemeral port: bind probe to learn one, then serve it.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let config = eval_usage::collector::CollectorConfig {
        port,
        store_root: dir.join("store"),
        token_file,
    };
    let handle = tokio::spawn(async move {
        let _ = eval_usage::collector::serve(config).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    (dir, port, handle)
}

fn post(port: u16, path: &str, body: &[u8], auth: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(token) = auth {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let text = String::from_utf8_lossy(&response).into_owned();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let payload = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, payload)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepts_batch_and_dedupes_redelivery() {
    let (_dir, port, handle) = spawn(None).await;
    let body = serde_json::to_vec(&serde_json::json!([event("a"), event("b")])).unwrap();
    let (status, payload) = post(port, "/v1/events", &body, None);
    assert_eq!(status, 200, "{payload}");
    assert!(payload.contains("\"accepted\":2"), "{payload}");
    let (status, payload) = post(port, "/v1/events", &body, None);
    assert_eq!(status, 200);
    assert!(payload.contains("\"duplicates\":2"), "{payload}");
    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejects_bad_json_and_bad_events() {
    let (_dir, port, handle) = spawn(None).await;
    let (status, _) = post(port, "/v1/events", b"{nope", None);
    assert_eq!(status, 400);
    let (status, _) = post(port, "/v1/events", b"{\"event_version\": 42}", None);
    assert_eq!(status, 400);
    let (status, _) = post(port, "/nope", b"{}", None);
    assert_eq!(status, 404);
    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_body_rejected() {
    let (_dir, port, handle) = spawn(None).await;
    let big = vec![b'x'; eval_usage::collector::MAX_BODY_BYTES + 1];
    let (status, _) = post(port, "/v1/events", &big, None);
    assert_eq!(status, 413, "{status}");
    handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_gate() {
    let dir = tmpdir("tok");
    let token_file = dir.join("token.txt");
    std::fs::write(&token_file, "s3cret\n").unwrap();
    let (_srv, port, handle) = spawn(Some(token_file)).await;
    let body = serde_json::to_vec(&event("t1")).unwrap();
    let (status, _) = post(port, "/v1/events", &body, None);
    assert_eq!(status, 401);
    let (status, _) = post(port, "/v1/events", &body, Some("wrong"));
    assert_eq!(status, 401);
    let (status, payload) = post(port, "/v1/events", &body, Some("s3cret"));
    assert_eq!(status, 200, "{payload}");
    handle.abort();
}

