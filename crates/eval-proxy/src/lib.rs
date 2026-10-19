//! Loopback reverse proxy for OpenAI-compatible `chat/completions`
//! calls. Real applications point their provider base URL at this
//! proxy; it injects the real provider key (the application only ever
//! sees a loopback URL, never key material), optionally swaps the
//! requested model, passes non-streaming and SSE responses through
//! unchanged, and records one `llm/v1` event per physical attempt in
//! the usage store — tokens only when the provider reports them.
//!
//! Budget: `max_requests` caps forwarded attempts. `max_cost_usd`
//! requires both per-1M prices and reserves a conservative cost band
//! *before* each request is forwarded (input tokens estimated from
//! wire size, output from `max_tokens` or a configured reserve), then
//! reconciles against provider-reported usage. Unpriced cost budgets
//! are refused at startup (fail closed). The reservation is an
//! application-side bound with documented slack — not a billing
//! guarantee — and is internal control state, never a metric.
//!
//! Server style matches the usage collector: hand-rolled loopback
//! HTTP/1.1 on tokio, small caps, no framework, no upstream framework
//! deps beyond an HTTP client (HTTPS to real providers is the need).

use blake3::Hasher;
use eval_usage::event::{EVENT_VERSION, EventOrigin, LlmEvent, Outcome, UsageContext};
use eval_usage::store::UsageStore;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub mod cassette;
pub use cassette::{
    CassetteMap, CassetteRecord, CassetteServeConfig, exchange_key, record_exchange,
};

pub const EVENT_NAME: &str = "llm.call";
/// Stable identity for derived event ids: fingerprint seed + sequence
/// slot + origin timestamp, truncated to the store's 32-hex key width.
pub fn derived_event_id(seed: &str, seq: u64, started_at_ms: u64) -> String {
    let mut hasher = Hasher::new();
    hasher.update(seed.as_bytes());
    hasher.update(&seq.to_le_bytes());
    hasher.update(&started_at_ms.to_le_bytes());
    hasher.finalize().to_hex()[..32].to_string()
}

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_TEE_BYTES: usize = 4 * 1024 * 1024;
pub const CONN_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_OUT_RESERVE_TOKENS: u64 = 1024;
pub const DEFAULT_UPSTREAM_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("proxy config: {0}")]
    Config(String),
    #[error("proxy io: {0}")]
    Io(String),
}

fn io<T: std::fmt::Display>(what: &str, error: T) -> ProxyError {
    ProxyError::Io(format!("{what}: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    Requests,
    Cost,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Loopback port (0 binds an ephemeral port — used by tests).
    pub port: u16,
    /// Upstream OpenAI-compatible base URL, e.g.
    /// `https://api.groq.com/openai/v1`.
    pub upstream: String,
    /// Env var holding the real provider key. The value is read once
    /// at startup and never logged, echoed, or stored.
    pub key_env: String,
    /// Project id stamped on every usage event.
    pub project: String,
    /// Provider label for usage events (GenAI `gen_ai.system` analog).
    pub provider_label: String,
    /// Optional tag stamped on every event.
    pub tag: Option<String>,
    /// Optional run id stamped on every event.
    pub run_id: Option<String>,
    pub context: UsageContext,
    pub origin: EventOrigin,
    /// Model swap map: requested model → served model.
    pub model_overrides: BTreeMap<String, String>,
    /// Max forwarded attempts; rejects beyond it (HTTP 429).
    pub max_requests: Option<u32>,
    /// Max cost (USD) over the proxy's lifetime; requires both prices
    /// (rejected at startup otherwise — fail closed).
    pub max_cost_usd: Option<f64>,
    pub price_in_per_1m: Option<f64>,
    pub price_out_per_1m: Option<f64>,
    /// Reserved output tokens per request when the caller does not
    /// declare `max_tokens`.
    pub out_reserve_tokens: u64,
    /// Usage store root (`<root>/usage/<project>/…`).
    pub store_root: PathBuf,
    pub upstream_timeout: Duration,
    /// When set, every successful (HTTP 200) upstream exchange is
    /// recorded as a cassette under this directory for offline replay.
    pub cassette_dir: Option<PathBuf>,
}

impl ProxyConfig {
    pub fn validate(&self) -> Result<(), ProxyError> {
        if self.project.trim().is_empty() {
            return Err(ProxyError::Config("project must be non-empty".into()));
        }
        if self.key_env.trim().is_empty() {
            return Err(ProxyError::Config("key_env must be non-empty".into()));
        }
        let upstream = self.upstream.trim();
        if !(upstream.starts_with("https://") || upstream.starts_with("http://")) {
            return Err(ProxyError::Config(
                "upstream must be an http(s) base URL".into(),
            ));
        }
        if self.provider_label.trim().is_empty() {
            return Err(ProxyError::Config(
                "provider label must be non-empty".into(),
            ));
        }
        if let Some(cap) = self.max_cost_usd {
            if cap < 0.0 {
                return Err(ProxyError::Config("max_cost_usd must be >= 0".into()));
            }
            let (Some(price_in), Some(price_out)) = (self.price_in_per_1m, self.price_out_per_1m)
            else {
                return Err(ProxyError::Config(
                    "fail closed: max_cost_usd requires price_in_per_1m and \
                     price_out_per_1m; an unpriced cost budget cannot bound real spend"
                        .into(),
                ));
            };
            if price_in < 0.0 || price_out < 0.0 {
                return Err(ProxyError::Config("prices must be >= 0".into()));
            }
        }
        if self.max_requests.is_some_and(|max| max == 0) {
            return Err(ProxyError::Config("max_requests must be > 0".into()));
        }
        Ok(())
    }
}

/// Money ledger: forwarded attempts, funds reserved by in-flight
/// requests, and reconciled spend.
#[derive(Debug)]
pub struct Budget {
    max_requests: Option<u32>,
    max_cost_usd: Option<f64>,
    state: Mutex<Ledger>,
}

#[derive(Debug, Default)]
struct Ledger {
    requests: u32,
    reserved_usd: f64,
    spent_usd: f64,
}

impl Budget {
    pub fn new(max_requests: Option<u32>, max_cost_usd: Option<f64>) -> Self {
        Self {
            max_requests,
            max_cost_usd,
            state: Mutex::new(Ledger::default()),
        }
    }

    /// Reserve one attempt. Returns the reserved amount (None when no
    /// cost budget is configured). Atomic against concurrent
    /// reservations: two requests can never jointly exceed the cap.
    pub fn reserve(&self, estimated_cost: Option<f64>) -> Result<Option<f64>, RejectReason> {
        let mut ledger = self.state.lock().map_err(|_| RejectReason::Requests)?;
        let mut counted = false;
        if let Some(max) = self.max_requests {
            if ledger.requests >= max {
                return Err(RejectReason::Requests);
            }
            ledger.requests += 1;
            counted = true;
        }
        match (self.max_cost_usd, estimated_cost) {
            (Some(_), Some(cost)) => {
                let cap = self.max_cost_usd.unwrap();
                if ledger.reserved_usd + ledger.spent_usd + cost > cap + f64::EPSILON {
                    if counted {
                        ledger.requests -= 1;
                    }
                    return Err(RejectReason::Cost);
                }
                ledger.reserved_usd += cost;
                Ok(Some(cost))
            }
            (Some(_), None) => {
                // Unpriced requests inside a priced budget: fail closed.
                if counted {
                    ledger.requests -= 1;
                }
                Err(RejectReason::Cost)
            }
            _ => Ok(None),
        }
    }

    /// Reconcile after the attempt finishes: release the reservation
    /// and charge actual spend. When usage was unobservable, charge
    /// the reservation itself — unknown use still consumes the
    /// conservative bound, never a silent zero.
    pub fn commit(&self, reservation: Option<f64>, actual_usd: f64) {
        let Ok(mut ledger) = self.state.lock() else {
            return;
        };
        if let Some(reserved) = reservation {
            ledger.reserved_usd = (ledger.reserved_usd - reserved).max(0.0);
        }
        ledger.spent_usd += actual_usd.max(0.0);
    }
}

/// Conservative internal reservation bound: wire bytes overestimate
/// tokens (JSON framing is not prompt text), which is deliberate — a
/// reservation may over-reserve, never under-reserve. Control state,
/// not a reported metric.
pub fn estimate_in_tokens(wire_bytes: usize) -> u64 {
    (wire_bytes as f64 / 3.0).ceil() as u64 + 32
}

pub fn cost_usd(in_tokens: u64, out_tokens: u64, price_in: f64, price_out: f64) -> f64 {
    in_tokens as f64 / 1_000_000.0 * price_in + out_tokens as f64 / 1_000_000.0 * price_out
}

struct ProxyState {
    config: ProxyConfig,
    key: String,
    store: UsageStore,
    budget: Budget,
    client: reqwest::Client,
    seq: AtomicU64,
}

/// Bind a loopback listener for `config` (ephemeral-port support is
/// for tests; the CLI uses [`serve`]).
pub async fn bind(config: &ProxyConfig) -> Result<(TcpListener, SocketAddr), ProxyError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = TcpListener::bind(addr).await.map_err(|e| io("bind", e))?;
    let bound = listener.local_addr().map_err(|e| io("local_addr", e))?;
    Ok((listener, bound))
}

/// Validate, bind, and serve until the future is cancelled.
pub async fn serve(config: ProxyConfig) -> Result<(), ProxyError> {
    config.validate()?;
    let (listener, _) = bind(&config).await?;
    serve_on(listener, config).await
}

/// Serve on an already-bound listener (shared path for CLI and tests).
pub async fn serve_on(listener: TcpListener, config: ProxyConfig) -> Result<(), ProxyError> {
    config.validate()?;
    let key = std::env::var(&config.key_env)
        .map_err(|_| ProxyError::Config(format!("key env {} is not set", config.key_env)))?
        .trim()
        .to_string();
    if key.is_empty() {
        return Err(ProxyError::Config(format!(
            "key env {} is empty",
            config.key_env
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(config.upstream_timeout)
        .build()
        .map_err(|e| io("http client", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| io("local_addr", e))?
        .port();
    eprintln!(
        "eval-proxy: 127.0.0.1:{port} -> {} (project {}); key from env {} never logged",
        config.upstream, config.project, config.key_env
    );
    let state = Arc::new(ProxyState {
        config: config.clone(),
        key,
        store: UsageStore::new(config.store_root.clone()),
        budget: Budget::new(config.max_requests, config.max_cost_usd),
        client,
        seq: AtomicU64::new(0),
    });
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| io("accept", e))?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle(state, stream).await;
        });
    }
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
    oversized: bool,
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, ProxyError> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await.map_err(|e| io("read", e))?;
        if n == 0 {
            return Err(ProxyError::Config("empty request".into()));
        }
        head.push(byte[0]);
        if head.len() > 64 * 1024 {
            return Err(ProxyError::Config("headers too large".into()));
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
    let mut content_length: usize = 0;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > MAX_BODY_BYTES {
        let mut drained = vec![0u8; MAX_BODY_BYTES + 1];
        let _ = stream.read_exact(&mut drained).await;
        return Ok(Request {
            method,
            path,
            body: Vec::new(),
            oversized: true,
        });
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| io("read body", e))?;
    }
    Ok(Request {
        method,
        path,
        body,
        oversized: false,
    })
}

async fn respond_stream(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), ProxyError> {
    let reason = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("Status");
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| io("write", e))?;
    stream.write_all(body).await.map_err(|e| io("write", e))?;
    stream.flush().await.map_err(|e| io("flush", e))
}

async fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), ProxyError> {
    respond_stream(stream, status, "application/json", body.as_bytes()).await
}

async fn respond_budget_reject(
    stream: &mut TcpStream,
    reason: RejectReason,
) -> Result<(), ProxyError> {
    let (status, kind) = match reason {
        RejectReason::Requests => (429u16, "request_budget_exhausted"),
        RejectReason::Cost => (402u16, "cost_budget_exhausted"),
    };
    respond_json(
        stream,
        status,
        &json!({"error": {"type": kind, "message": "eval budget exhausted before forwarding"}})
            .to_string(),
    )
    .await
}

async fn handle(state: Arc<ProxyState>, mut stream: TcpStream) -> Result<(), ProxyError> {
    let request = match tokio::time::timeout(CONN_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        _ => {
            return respond_json(&mut stream, 400, r#"{"error":"bad request"}"#).await;
        }
    };
    if request.oversized {
        return respond_json(&mut stream, 413, r#"{"error":"body too large"}"#).await;
    }
    if request.method != "POST" {
        return respond_json(&mut stream, 405, r#"{"error":"POST only"}"#).await;
    }
    let normalized = request.path.trim_end_matches('/');
    if normalized != "/v1/chat/completions" && normalized != "/chat/completions" {
        return respond_json(&mut stream, 404, r#"{"error":"not found"}"#).await;
    }
    let mut body: Value = match serde_json::from_slice::<Value>(&request.body) {
        Ok(value @ Value::Object(_)) => value,
        _ => {
            return respond_json(
                &mut stream,
                400,
                r#"{"error":{"message":"body must be a JSON object"}}"#,
            )
            .await;
        }
    };
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let out_reserve = body
        .get("max_tokens")
        .and_then(Value::as_u64)
        .or_else(|| body.get("max_completion_tokens").and_then(Value::as_u64))
        .unwrap_or(state.config.out_reserve_tokens)
        .max(1);
    let est_in = estimate_in_tokens(request.body.len());
    let prices = match (state.config.price_in_per_1m, state.config.price_out_per_1m) {
        (Some(price_in), Some(price_out)) => Some((price_in, price_out)),
        _ => None,
    };
    let estimated_cost = prices
        .as_ref()
        .map(|(price_in, price_out)| cost_usd(est_in, out_reserve, *price_in, *price_out));
    let reservation = match state.budget.reserve(estimated_cost) {
        Ok(reservation) => reservation,
        Err(reason) => return respond_budget_reject(&mut stream, reason).await,
    };
    let served_model = state.config.model_overrides.get(&requested_model).cloned();
    if let Some(model) = &served_model {
        body["model"] = json!(model);
    }
    if streaming {
        let has_usage_flag = body
            .get("stream_options")
            .and_then(Value::as_object)
            .is_some_and(|options| options.contains_key("include_usage"));
        if !has_usage_flag {
            body["stream_options"] = json!({"include_usage": true});
        }
    }
    let corpus = match serde_json::to_vec(&body) {
        Ok(corpus) => corpus,
        Err(_) => {
            state.budget.commit(reservation, 0.0);
            return respond_json(&mut stream, 500, r#"{"error":"encode failed"}"#).await;
        }
    };
    let seq = state.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let started_at_ms = now_ms();
    let started = Instant::now();
    let context = EventContext {
        requested_model,
        served_model,
        fingerprint: cassette::exchange_key(&request.body),
        seq,
        started_at_ms,
        started,
    };
    let url = format!(
        "{}/chat/completions",
        state.config.upstream.trim_end_matches('/')
    );
    let response = state
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {}", state.key))
        .header("content-type", "application/json")
        .body(corpus.clone())
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let (outcome, code) = if error.is_timeout() {
                (Outcome::Timeout, "upstream_timeout")
            } else {
                (Outcome::Error, "upstream_connect")
            };
            state.budget.commit(reservation, 0.0);
            emit_event(
                &state,
                &context,
                streaming,
                None,
                outcome,
                Some(code),
                true,
                None,
            );
            return respond_json(
                &mut stream,
                if code == "upstream_timeout" { 504 } else { 502 },
                &json!({"error": {"type": code, "message": "upstream unreachable"}}).to_string(),
            )
            .await;
        }
    };
    let status = response.status().as_u16();
    let upstream_content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    if status != 200 {
        let bytes = response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .unwrap_or_default();
        state.budget.commit(reservation, 0.0);
        emit_event(
            &state,
            &context,
            streaming,
            None,
            Outcome::Error,
            Some(&format!("http_{status}")),
            matches!(status, 408 | 429 | 500 | 502 | 503 | 504),
            None,
        );
        respond_stream(&mut stream, status, &upstream_content_type, &bytes).await
    } else if streaming && upstream_content_type.contains("event-stream") {
        stream_response(state, response, &mut stream, context, corpus).await
    } else {
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(_) => {
                state.budget.commit(reservation, 0.0);
                emit_event(
                    &state,
                    &context,
                    streaming,
                    None,
                    Outcome::Error,
                    Some("upstream_read"),
                    true,
                    None,
                );
                return respond_json(
                    &mut stream,
                    502,
                    r#"{"error":{"type":"upstream_read","message":"upstream response unreadable"}}"#,
                )
                .await;
            }
        };
        let usage = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value.get("usage").cloned())
            .map(|usage| parse_usage(&usage));
        let actual = match (prices, usage.as_ref()) {
            (Some((price_in, price_out)), Some(usage)) => cost_usd(
                usage.input.unwrap_or(est_in),
                usage.output.unwrap_or(0),
                price_in,
                price_out,
            ),
            (Some(_), None) => reservation.unwrap_or(0.0),
            (None, _) => 0.0,
        };
        state.budget.commit(reservation, actual);
        emit_event(
            &state,
            &context,
            false,
            usage.as_ref(),
            Outcome::Ok,
            None,
            false,
            None,
        );
        record_cassette(
            &state,
            &context,
            &corpus,
            false,
            &upstream_content_type,
            &bytes,
        );
        respond_stream(&mut stream, status, &upstream_content_type, &bytes).await
    }
}

struct EventContext {
    requested_model: String,
    served_model: Option<String>,
    /// Fingerprint of the application's exact request body (used as
    /// the event-seed id and the cassette match key).
    fingerprint: String,
    seq: u64,
    started_at_ms: u64,
    started: Instant,
}

#[allow(clippy::too_many_arguments)]
fn emit_event(
    state: &Arc<ProxyState>,
    context: &EventContext,
    streaming: bool,
    usage: Option<&ReportedUsage>,
    outcome: Outcome,
    error_code: Option<&str>,
    retriable: bool,
    ttft: Option<u64>,
) {
    let served = context
        .served_model
        .as_deref()
        .unwrap_or(&context.requested_model);
    let mut extensions = HashMap::new();
    extensions.insert("proxy".to_string(), json!(true));
    if served != context.requested_model {
        extensions.insert(
            "requested_model".to_string(),
            json!(context.requested_model),
        );
    }
    let event_id = derived_event_id(&context.fingerprint, context.seq, context.started_at_ms);
    let event = LlmEvent {
        event: "llm.call".to_string(),
        event_version: EVENT_VERSION,
        event_id,
        project: state.config.project.clone(),
        trace_id: None,
        task_id: None,
        request_id: format!("px-{}", context.seq),
        attempt: 0,
        provider: state.config.provider_label.clone(),
        model: served.to_string(),
        started_at_ms: context.started_at_ms,
        duration_ms: Some(context.started.elapsed().as_millis() as u64),
        streaming,
        ttft_ms: ttft,
        input_tokens: usage.and_then(|u| u.input),
        output_tokens: usage.and_then(|u| u.output),
        cached_tokens: usage.and_then(|u| u.cached),
        reasoning_tokens: usage.and_then(|u| u.reasoning),
        outcome,
        error_code: error_code.map(str::to_string),
        retriable,
        cost_usd: None,
        cost_provenance: None,
        prompt_id: None,
        prompt_hash: None,
        run_id: state.config.run_id.clone(),
        case_id: None,
        tag: state.config.tag.clone(),
        context: state.config.context,
        origin: state.config.origin,
        extensions: Some(extensions),
    };
    let Ok(line) = serde_json::to_string(&event) else {
        return;
    };
    match state.store.import_str(
        &state.config.project,
        &line,
        state.config.tag.as_deref(),
        state.config.run_id.as_deref(),
    ) {
        Ok(_) => {}
        Err(error) => eprintln!("eval-proxy: event store rejected: {error}"),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ReportedUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: Option<u64>,
    pub reasoning: Option<u64>,
}

fn parse_usage(usage: &Value) -> ReportedUsage {
    let number = |container: Option<&Value>, key: &str| {
        container
            .and_then(|value| value.get(key))
            .and_then(Value::as_u64)
    };
    ReportedUsage {
        input: number(Some(usage), "prompt_tokens").or(number(Some(usage), "input_tokens")),
        output: number(Some(usage), "completion_tokens").or(number(Some(usage), "output_tokens")),
        cached: number(usage.get("prompt_tokens_details"), "cached_tokens"),
        reasoning: number(usage.get("completion_tokens_details"), "reasoning_tokens"),
    }
}

/// Extract the last `"usage":{...}` object from an SSE byte stream.
fn find_last_usage(tee: &[u8]) -> Option<ReportedUsage> {
    let needle = b"\"usage\":";
    let mut end = tee.len();
    while let Some(pos) = tee[..end]
        .windows(needle.len())
        .rposition(|window| window == needle)
    {
        let mut cursor = pos + needle.len();
        while cursor < tee.len() && (tee[cursor] as char).is_whitespace() {
            cursor += 1;
        }
        if cursor < tee.len() && tee[cursor] == b'{' {
            if let Some(stop) = json_object_end(tee, cursor) {
                if let Ok(value) = serde_json::from_slice::<Value>(&tee[cursor..stop]) {
                    return Some(parse_usage(&value));
                }
            }
        }
        end = pos;
    }
    None
}

fn json_object_end(bytes: &[u8], brace_start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes[brace_start..].iter().enumerate() {
        let byte = *byte;
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(brace_start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn record_cassette(
    state: &Arc<ProxyState>,
    context: &EventContext,
    forwarded_body: &[u8],
    streaming: bool,
    content_type: &str,
    response_body: &[u8],
) {
    let Some(dir) = state.config.cassette_dir.as_ref() else {
        return;
    };
    let served = context
        .served_model
        .as_deref()
        .unwrap_or(&context.requested_model);
    match cassette::record_exchange(
        dir,
        &context.fingerprint,
        forwarded_body,
        &context.requested_model,
        served,
        streaming,
        content_type,
        response_body,
    ) {
        Ok(true) | Ok(false) => {}
        Err(error) => eprintln!("eval-proxy: cassette record failed: {error}"),
    }
}

async fn stream_response(
    state: Arc<ProxyState>,
    response: reqwest::Response,
    client_stream: &mut TcpStream,
    context: EventContext,
    forwarded_body: Vec<u8>,
) -> Result<(), ProxyError> {
    let head = "\r\nHTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        .trim_start()
        .to_string();
    client_stream
        .write_all(head.as_bytes())
        .await
        .map_err(|e| io("write", e))?;
    let mut upstream = pin!(response.bytes_stream());
    let mut tee: Vec<u8> = Vec::new();
    let mut tee_complete = true;
    let mut ttft: Option<u64> = None;
    let mut outcome = Outcome::Ok;
    let mut error_code: Option<String> = None;
    loop {
        let chunk = match upstream.next().await {
            Some(Ok(chunk)) => chunk,
            Some(Err(_)) => {
                outcome = Outcome::Error;
                error_code = Some("upstream_stream_interrupted".to_string());
                break;
            }
            None => break,
        };
        if ttft.is_none() {
            ttft = Some(context.started.elapsed().as_millis() as u64);
        }
        if tee.len().saturating_add(chunk.len()) <= MAX_TEE_BYTES {
            tee.extend_from_slice(&chunk);
        } else {
            tee_complete = false;
        }
        let chunk_len = chunk.len();
        client_stream
            .write_all(format!("{chunk_len:x}\r\n").as_bytes())
            .await
            .map_err(|e| io("write", e))?;
        client_stream
            .write_all(&chunk)
            .await
            .map_err(|e| io("write", e))?;
        client_stream
            .write_all(b"\r\n")
            .await
            .map_err(|e| io("write", e))?;
    }
    client_stream
        .write_all(b"0\r\n\r\n")
        .await
        .map_err(|e| io("write", e))?;
    client_stream.flush().await.map_err(|e| io("flush", e))?;
    let usage = if outcome == Outcome::Ok {
        find_last_usage(&tee)
    } else {
        None
    };
    emit_event(
        &state,
        &context,
        true,
        usage.as_ref(),
        outcome,
        error_code.as_deref(),
        outcome != Outcome::Ok,
        ttft,
    );
    if outcome == Outcome::Ok && tee_complete {
        record_cassette(
            &state,
            &context,
            &forwarded_body,
            true,
            "text/event-stream",
            &tee,
        );
    }
    Ok(())
}
