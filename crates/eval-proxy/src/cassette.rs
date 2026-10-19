//! Exact-exchange recording + replay ("cassettes"): a recording proxy
//! run captures every successful `chat/completions` exchange, and the
//! replay server serves those recorded responses to the same
//! application offline — a deterministic rerun of an observed
//! session. Replay behavior is bounded and honest by design:
//!
//! - Lookup is a fingerprint of the application's own request body
//!   (blake3); records only exist for successful (HTTP 200) upstream
//!   exchanges.
//! - An unmatched request fails loudly (HTTP 404 `cassette_miss`) —
//!   never a silent fallback to a plausible-looking answer.
//! - Records hold request/response bodies only. Authorization headers
//!   and proxy key material are never written.
//! - Every replay hit stamps one `llm/v1` event with
//!   `origin: replay`, sourced from the usage inside the recorded
//!   response (cassette runs therefore also produce real usage
//!   evidence — of the *recorded* calls, so declared scarcity stays
//!   visible).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpListener;

use crate::{
    CONN_TIMEOUT, ProxyError, ReportedUsage, derived_event_id, find_last_usage, io, now_ms,
    parse_usage, read_request, respond_json, respond_stream,
};
use eval_usage::event::{EVENT_VERSION, EventOrigin, LlmEvent, Outcome, UsageContext};
use eval_usage::store::UsageStore;

pub const CASSETTE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CassetteRecord {
    pub cassette_version: u32,
    pub id: String,
    pub recorded_at_ms: u64,
    pub requested_model: String,
    pub served_model: String,
    pub streaming: bool,
    pub content_type: String,
    /// Forwarded (post model-swap / usage-flag injection) request
    /// body. Transparency only; matching uses `id`.
    pub request_body: String,
    pub response_body: String,
}

/// Match key: blake3 hex of the application's exact request body.
pub fn exchange_key(request_wire: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(request_wire);
    hasher.finalize().to_hex()[..48].to_string()
}

/// Write one cassette file. `id` is the caller's known fingerprint
/// (see [`exchange_key`]). First-write-wins: a repeated identical
/// request keeps its originally recorded response so replays stay
/// deterministic. `Ok(false)` when a record already exists.
#[allow(clippy::too_many_arguments)]
pub fn record_exchange(
    dir: &Path,
    id: &str,
    forwarded_body: &[u8],
    requested_model: &str,
    served_model: &str,
    streaming: bool,
    content_type: &str,
    response_body: &[u8],
) -> Result<bool, ProxyError> {
    std::fs::create_dir_all(dir).map_err(|e| io("cassette dir", e))?;
    let path = dir.join(format!("{id}.json"));
    if path.exists() {
        return Ok(false);
    }
    let record = CassetteRecord {
        cassette_version: CASSETTE_VERSION,
        id: id.to_string(),
        recorded_at_ms: now_ms(),
        requested_model: requested_model.to_string(),
        served_model: served_model.to_string(),
        streaming,
        content_type: content_type.to_string(),
        request_body: String::from_utf8_lossy(forwarded_body).into_owned(),
        response_body: String::from_utf8_lossy(response_body).into_owned(),
    };
    let json = serde_json::to_vec_pretty(&record).map_err(|e| io("cassette encode", e))?;
    let tmp = dir.join(format!("{id}.tmp"));
    std::fs::write(&tmp, json).map_err(|e| io("cassette write", e))?;
    std::fs::rename(&tmp, &path).map_err(|e| io("cassette rename", e))?;
    Ok(true)
}

#[derive(Debug, Clone)]
pub struct CassetteServeConfig {
    /// Loopback port (0 binds an ephemeral port — used by tests).
    pub port: u16,
    pub dir: PathBuf,
    pub project: String,
    pub provider_label: String,
    pub tag: Option<String>,
    pub run_id: Option<String>,
    pub context: UsageContext,
    pub store_root: PathBuf,
}

#[derive(Debug, Default)]
pub struct CassetteMap(BTreeMap<String, CassetteRecord>);

impl CassetteMap {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&CassetteRecord> {
        self.0.get(id)
    }
}

/// Load every `*.json` cassette under `dir`. Divergent duplicates
/// (same fingerprint, different response) load as an error — replay
/// must be deterministic without depending on directory order.
pub fn load_dir(dir: &Path) -> Result<CassetteMap, ProxyError> {
    let entries = std::fs::read_dir(dir).map_err(|e| io("cassette dir", e))?;
    let mut map = CassetteMap::default();
    for entry in entries {
        let entry = entry.map_err(|e| io("cassette dir entry", e))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| io("cassette read", e))?;
        let record: CassetteRecord = serde_json::from_slice(&bytes)
            .map_err(|e| ProxyError::Config(format!("cassette {}: {e}", path.display())))?;
        if record.cassette_version != CASSETTE_VERSION {
            return Err(ProxyError::Config(format!(
                "cassette {}: version {} unsupported",
                path.display(),
                record.cassette_version
            )));
        }
        match map.0.get(&record.id) {
            Some(existing) if existing.response_body != record.response_body => {
                return Err(ProxyError::Config(format!(
                    "cassette fingerprint {} recorded twice with divergent responses",
                    record.id
                )));
            }
            Some(_) => continue,
            None => {}
        }
        map.0.insert(record.id.clone(), record);
    }
    if map.0.is_empty() {
        return Err(ProxyError::Config(format!(
            "no cassettes found in {}",
            dir.display()
        )));
    }
    Ok(map)
}

struct CassetteServer {
    config: CassetteServeConfig,
    map: CassetteMap,
    store: UsageStore,
    seq: AtomicU64,
}

/// Bind a loopback listener for the replay server.
pub async fn bind(config: &CassetteServeConfig) -> Result<(TcpListener, SocketAddr), ProxyError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = TcpListener::bind(addr).await.map_err(|e| io("bind", e))?;
    let bound = listener.local_addr().map_err(|e| io("local_addr", e))?;
    Ok((listener, bound))
}

pub async fn serve(config: CassetteServeConfig) -> Result<(), ProxyError> {
    let (listener, _) = bind(&config).await?;
    serve_on(listener, config).await
}

pub async fn serve_on(
    listener: TcpListener,
    config: CassetteServeConfig,
) -> Result<(), ProxyError> {
    if config.project.trim().is_empty() {
        return Err(ProxyError::Config("project must be non-empty".into()));
    }
    let map = load_dir(&config.dir)?;
    let count = map.len();
    let port = listener
        .local_addr()
        .map_err(|e| io("local_addr", e))?
        .port();
    let state = Arc::new(CassetteServer {
        config: config.clone(),
        map,
        store: UsageStore::new(config.store_root.clone()),
        seq: AtomicU64::new(0),
    });
    eprintln!(
        "eval-cassette: 127.0.0.1:{port} replaying {} exchange(s) from {}; hits stamp origin=replay",
        count,
        config.dir.display()
    );
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| io("accept", e))?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle(state, stream).await;
        });
    }
}

async fn handle(
    server: Arc<CassetteServer>,
    mut stream: tokio::net::TcpStream,
) -> Result<(), ProxyError> {
    let request = match tokio::time::timeout(CONN_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        _ => return respond_json(&mut stream, 400, r#"{"error":"bad request"}"#).await,
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
    let body: Value = match serde_json::from_slice::<Value>(&request.body) {
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
    let started_at_ms = now_ms();
    let started = std::time::Instant::now();
    let id = exchange_key(&request.body);
    let Some(record) = server.map.get(&id) else {
        let line = miss_event(
            &server,
            &requested_model,
            started_at_ms,
            started.elapsed().as_millis() as u64,
        );
        if let Ok(json) = serde_json::to_string(&line) {
            let _ = server.store.import_str(
                &server.config.project,
                &json,
                server.config.tag.as_deref(),
                server.config.run_id.as_deref(),
            );
        }
        return respond_json(
            &mut stream,
            404,
            r#"{"error":{"type":"cassette_miss","message":"no cassette recorded for this request fingerprint"}}"#,
        )
        .await;
    };
    if record.streaming != streaming {
        let mismatch = format!(
            "cassette_miss_streaming_mismatch: recorded streaming={} requested streaming={}",
            record.streaming, streaming
        );
        return respond_json(
            &mut stream,
            409,
            &serde_json::json!({"error": {"type": mismatch}}).to_string(),
        )
        .await;
    }
    let usage = if record.streaming {
        find_last_usage(record.response_body.as_bytes())
    } else {
        serde_json::from_slice::<Value>(record.response_body.as_bytes())
            .ok()
            .and_then(|value| value.get("usage").cloned())
            .map(|usage| parse_usage(&usage))
    };
    let line = hit_event(
        &server,
        record,
        usage.as_ref(),
        started_at_ms,
        started.elapsed().as_millis() as u64,
    );
    if let Ok(json) = serde_json::to_string(&line) {
        if let Err(error) = server.store.import_str(
            &server.config.project,
            &json,
            server.config.tag.as_deref(),
            server.config.run_id.as_deref(),
        ) {
            eprintln!("eval-cassette: event store rejected: {error}");
        }
    }
    respond_stream(
        &mut stream,
        200,
        &record.content_type,
        record.response_body.as_bytes(),
    )
    .await
}

fn hit_event(
    server: &Arc<CassetteServer>,
    record: &CassetteRecord,
    usage: Option<&ReportedUsage>,
    started_at_ms: u64,
    duration_ms: u64,
) -> LlmEvent {
    let seq = server.seq.fetch_add(1, Ordering::SeqCst);
    let mut extensions = std::collections::HashMap::new();
    extensions.insert("cassette".to_string(), serde_json::json!(true));
    extensions.insert("fingerprint".to_string(), serde_json::json!(record.id));
    let event_id = derived_event_id(&record.id, seq, started_at_ms);
    LlmEvent {
        event: "llm.call".to_string(),
        event_version: EVENT_VERSION,
        event_id,
        project: server.config.project.clone(),
        trace_id: None,
        task_id: None,
        request_id: format!("cas-{seq}"),
        attempt: 0,
        provider: server.config.provider_label.clone(),
        model: record.served_model.clone(),
        started_at_ms,
        duration_ms: Some(duration_ms),
        streaming: record.streaming,
        ttft_ms: None,
        input_tokens: usage.and_then(|u| u.input),
        output_tokens: usage.and_then(|u| u.output),
        cached_tokens: usage.and_then(|u| u.cached),
        reasoning_tokens: usage.and_then(|u| u.reasoning),
        outcome: Outcome::Ok,
        error_code: None,
        retriable: false,
        cost_usd: None,
        cost_provenance: None,
        prompt_id: None,
        prompt_hash: None,
        run_id: server.config.run_id.clone(),
        case_id: None,
        tag: server.config.tag.clone(),
        context: server.config.context,
        origin: EventOrigin::Replay,
        extensions: Some(extensions),
    }
}

fn miss_event(
    server: &Arc<CassetteServer>,
    requested_model: &str,
    started_at_ms: u64,
    duration_ms: u64,
) -> LlmEvent {
    let seq = server.seq.fetch_add(1, Ordering::SeqCst);
    let mut extensions = std::collections::HashMap::new();
    extensions.insert("cassette".to_string(), serde_json::json!(true));
    extensions.insert("cassette_miss".to_string(), serde_json::json!(true));
    let event_id = derived_event_id("miss", seq, started_at_ms);
    LlmEvent {
        event: "llm.call".to_string(),
        event_version: EVENT_VERSION,
        event_id,
        project: server.config.project.clone(),
        trace_id: None,
        task_id: None,
        request_id: format!("cas-{seq}"),
        attempt: 0,
        provider: server.config.provider_label.clone(),
        model: requested_model.to_string(),
        started_at_ms,
        duration_ms: Some(duration_ms),
        streaming: false,
        ttft_ms: None,
        input_tokens: None,
        output_tokens: None,
        cached_tokens: None,
        reasoning_tokens: None,
        outcome: Outcome::Error,
        error_code: Some("cassette_miss".to_string()),
        retriable: false,
        cost_usd: None,
        cost_provenance: None,
        prompt_id: None,
        prompt_hash: None,
        run_id: server.config.run_id.clone(),
        case_id: None,
        tag: server.config.tag.clone(),
        context: server.config.context,
        origin: EventOrigin::Replay,
        extensions: Some(extensions),
    }
}
