//! Minimal loopback HTTP collector for usage events.
//!
//! POST /v1/events with a JSON body holding one event or an array of
//! events. This is intentionally small: loopback bind only, 1 MiB
//! body cap, per-connection timeouts, a simple rate cap, optional
//! bearer token. It is a local convenience inlet — the same
//! validated, idempotent import path as file ingestion — not a
//! hosted service.

use crate::error::UsageError;
use crate::event::LlmEvent;
use crate::store::{ImportSummary, UsageStore};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 1 MiB body cap.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Per-connection read timeout.
pub const CONN_TIMEOUT: Duration = Duration::from_secs(10);
/// Rate cap: 100 requests per 10 s window.
pub const RATE_LIMIT: usize = 100;
pub const RATE_WINDOW: Duration = Duration::from_secs(10);
/// Max concurrent connections.
pub const MAX_CONN: usize = 64;

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    pub port: u16,
    pub store_root: PathBuf,
    /// Optional shared-token file. Absent = loopback without auth
    /// (documented; never bind this beyond loopback).
    pub token_file: Option<PathBuf>,
}

impl CollectorConfig {
    /// Loopback bind address. Non-loopback binds are refused.
    pub fn bind_addr(&self) -> Result<std::net::SocketAddr, UsageError> {
        Ok(std::net::SocketAddr::from(([127, 0, 0, 1], self.port)))
    }

    fn expected_token(&self) -> Result<Option<String>, UsageError> {
        match &self.token_file {
            Some(path) => {
                let raw = std::fs::read_to_string(path)
                    .map_err(|e| UsageError::io(path.display().to_string(), e))?;
                Ok(Some(raw.trim().to_string()))
            }
            None => Ok(None),
        }
    }
}

struct RateGate {
    window_start: Instant,
    count: usize,
}

impl RateGate {
    fn allow(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.window_start) > RATE_WINDOW {
            self.window_start = now;
            self.count = 0;
        }
        self.count += 1;
        self.count <= RATE_LIMIT
    }
}

/// Serve forever (until the returned future is dropped/cancelled).
pub async fn serve(config: CollectorConfig) -> Result<(), UsageError> {
    let addr = config.bind_addr()?;
    let token = config.expected_token()?;
    let store = Arc::new(UsageStore::new(config.store_root));
    let gate = Arc::new(Mutex::new(RateGate {
        window_start: Instant::now(),
        count: 0,
    }));
    let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONN));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| UsageError::io(addr.to_string(), e))?;
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| UsageError::io(addr.to_string(), e))?;
        let store = store.clone();
        let gate = gate.clone();
        let sem = sem.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            if let Err(e) = handle(stream, &store, &gate, token.as_deref()).await {
                let _ = e;
            }
        });
    }
}

async fn handle(
    mut stream: tokio::net::TcpStream,
    store: &UsageStore,
    gate: &Mutex<RateGate>,
    token: Option<&str>,
) -> Result<(), UsageError> {
    let result = tokio::time::timeout(CONN_TIMEOUT, read_request(&mut stream)).await;
    let (method, path, headers, body, oversized) = match result {
        Ok(Ok(request)) => request,
        _ => {
            return respond(&mut stream, 400, r#"{"error":"bad request"}"#).await;
        }
    };
    if oversized {
        return respond(&mut stream, 413, r#"{"error":"body too large"}"#).await;
    }
    if method != "POST" || path != "/v1/events" {
        return respond(&mut stream, 404, r#"{"error":"not found"}"#).await;
    }
    if !gate.lock().map(|mut g| g.allow()).unwrap_or(false) {
        return respond(&mut stream, 429, r#"{"error":"rate limited"}"#).await;
    }
    if let Some(expected) = token {
        let got = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        if !constant_time_eq(got.as_bytes(), format!("Bearer {expected}").as_bytes()) {
            return respond(&mut stream, 401, r#"{"error":"unauthorized"}"#).await;
        }
    }
    let events: Vec<LlmEvent> = match serde_json::from_slice(&body) {
        Ok(value) => match value {
            serde_json::Value::Array(items) => {
                let mut parsed = Vec::with_capacity(items.len());
                for item in items {
                    match serde_json::from_value(item) {
                        Ok(event) => parsed.push(event),
                        Err(e) => {
                            return respond(
                                &mut stream,
                                400,
                                &format!("{{\"error\":\"bad event: {e}\"}}"),
                            )
                            .await;
                        }
                    }
                }
                parsed
            }
            single => match serde_json::from_value(single) {
                Ok(event) => vec![event],
                Err(e) => {
                    return respond(&mut stream, 400, &format!("{{\"error\":\"bad event: {e}\"}}"))
                        .await;
                }
            },
        },
        Err(e) => {
            return respond(&mut stream, 400, &format!("{{\"error\":\"bad json: {e}\"}}")).await;
        }
    };
    // Group by project so one batch can carry several projects; each
    // group imports through the idempotent path.
    let mut by_project: std::collections::BTreeMap<String, Vec<LlmEvent>> =
        std::collections::BTreeMap::new();
    for event in events {
        by_project.entry(event.project.clone()).or_default().push(event);
    }
    let mut total = ImportSummary::default();
    for (project, group) in &by_project {
        let body = group
            .iter()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        match store.import_str(project, &body, None) {
            Ok(summary) => {
                total.accepted += summary.accepted;
                total.duplicates += summary.duplicates;
                total.rejected.extend(summary.rejected);
            }
            Err(e) => {
                return respond(&mut stream, 400, &format!("{{\"error\":\"{e}\"}}")).await;
            }
        }
    }
    let payload = serde_json::to_string(&total)
        .unwrap_or_else(|_| r#"{"error":"encode"}"#.to_string());
    respond(&mut stream, 200, &payload).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

type Headers = Vec<(String, String)>;

async fn read_request(
    stream: &mut tokio::net::TcpStream,
) -> Result<(String, String, Headers, Vec<u8>, bool), UsageError> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| UsageError::io("read".to_string(), e))?;
        if n == 0 {
            return Err(UsageError::Config("empty request".to_string()));
        }
        head.push(byte[0]);
        if head.len() > 64 * 1024 {
            return Err(UsageError::Config("headers too large".to_string()));
        }
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head);
    let mut lines = head_text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers: Headers = Vec::new();
    let mut content_length: usize = 0;
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
    if content_length > MAX_BODY_BYTES {
        // Drain up to the cap so the client receives the 413 instead
        // of a reset; never read unboundedly.
        let mut drained = vec![0u8; MAX_BODY_BYTES + 1];
        let _ = stream.read_exact(&mut drained).await;
        return Ok((method, path, headers, Vec::new(), true));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| UsageError::io("read body".to_string(), e))?;
    }
    Ok((method, path, headers, body, false))
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: &str,
) -> Result<(), UsageError> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| UsageError::io("write".to_string(), e))
}
