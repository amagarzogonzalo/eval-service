//! `eval-protocol`: versioned `eval/v1` subprocess wire contract.
//!
//! M1 scope only: message types, validation, version negotiation,
//! tolerant-runner / strict-target parsing, and ordering helpers.
//! No process spawning, no grading, no storage (see `docs/02-architecture.md`
//! and `docs/03-protocol-v1.md`).
//!
//! Transport recap (normative spec is `docs/03-protocol-v1.md`):
//! - Runner writes to target stdin, reads target stdout as JSONL.
//! - stdout: JSONL only, one message per line, UTF-8, `\n`-terminated,
//!   max [`MAX_LINE_BYTES`] (8 MiB) per line.
//! - stderr: free-form logs, never parsed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Protocol identifier. Any other major version is a negotiation failure.
pub const PROTOCOL_VERSION: &str = "eval/v1";
/// Maximum accepted stdout line size (8 MiB). Larger is a `ProtocolError`.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Timeout rule factor: runner deadline must be >= 1.5x the target's own
/// internal wall budget so graceful exhaustion and runner kills stay
/// distinguishable (see `docs/03-protocol-v1.md`).
pub const TIMEOUT_RULE_FACTOR: f64 = 1.5;

// ---------------------------------------------------------------------------
// Modes & handshake
// ---------------------------------------------------------------------------

/// Execution mode negotiated in the handshake.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Persistent process, N cases per connection (default).
    #[default]
    Worker,
    /// Stateless: handshake -> one case -> terminal message -> exit 0.
    Oneshot,
}

/// First stdin line, runner -> target.
///
/// NOTE: no `"type"` discriminator by design (see spec). The runner is
/// tolerant of unknown fields; the target must be strict
/// (`deny_unknown_fields`). This struct is the tolerant (runner-side)
/// view — use [`parse_handshake_strict`] to emulate the target side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Handshake {
    /// Must equal [`PROTOCOL_VERSION`].
    pub protocol: String,
    /// Defaults to worker when absent (tolerant path).
    #[serde(default)]
    pub mode: Mode,
}

impl Handshake {
    pub fn new(mode: Mode) -> Self {
        Self {
            protocol: PROTOCOL_VERSION.to_string(),
            mode,
        }
    }
}

/// Negotiate the mode from a handshake. Runtime deserialization check,
/// not a compile-time claim: any other major version aborts.
pub fn negotiate(handshake: &Handshake) -> Result<Mode, ProtocolError> {
    if handshake.protocol == PROTOCOL_VERSION {
        Ok(handshake.mode)
    } else {
        Err(ProtocolError::VersionMismatch {
            expected: PROTOCOL_VERSION.to_string(),
            received: handshake.protocol.clone(),
        })
    }
}

/// Tolerant handshake parse (runner side): unknown fields ignored.
pub fn parse_handshake(line: &str) -> Result<Handshake, ProtocolError> {
    check_line_size(line)?;
    serde_json::from_str::<Handshake>(line).map_err(|e| ProtocolError::Json(e.to_string()))
}

/// Strict handshake parse (target side): unknown fields rejected.
pub fn parse_handshake_strict(line: &str) -> Result<HandshakeStrict, ProtocolError> {
    check_line_size(line)?;
    serde_json::from_str::<HandshakeStrict>(line).map_err(|e| ProtocolError::Json(e.to_string()))
}

/// Strict target-side handshake view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandshakeStrict {
    pub protocol: String,
    pub mode: Mode,
}

// ---------------------------------------------------------------------------
// Runner -> target: case / shutdown
// ---------------------------------------------------------------------------

/// Generic input shape (not agent-specific).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Input {
    /// e.g. `["eval", "output/build/cv_ink.pdf"]`.
    #[serde(default)]
    pub argv: Vec<String>,
    /// Text fed on stdin by the adapter.
    #[serde(default)]
    pub stdin_text: String,
    /// Open structured params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Per-case limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Limits {
    /// Per-case deadline in ms. Must be >= 1.
    pub timeout_ms: u64,
    /// Max captured output bytes. Must be >= 1.
    pub max_output_bytes: u64,
}

/// Runner -> target evaluation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CaseRequest {
    /// Always `"case"`.
    #[serde(rename = "type")]
    pub kind: CaseKind,
    pub case_id: String,
    #[serde(default)]
    pub input: Input,
    pub limits: Limits,
    /// Allowlisted env only (never a full dump). Absent == empty.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CaseKind {
    Case,
}

/// Runner -> target (worker mode only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shutdown {
    #[serde(rename = "type")]
    pub kind: ShutdownKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ShutdownKind {
    Shutdown,
}

/// Strict target-side case view (unknown fields rejected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaseRequestStrict {
    #[serde(rename = "type")]
    pub kind: CaseKind,
    pub case_id: String,
    pub input: InputStrict,
    pub limits: Limits,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Strict input view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputStrict {
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub stdin_text: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Tolerant case parse (runner test helpers).
pub fn parse_case(line: &str) -> Result<CaseRequest, ProtocolError> {
    check_line_size(line)?;
    serde_json::from_str::<CaseRequest>(line).map_err(|e| ProtocolError::Json(e.to_string()))
}

/// Strict case parse (what a conforming target must enforce).
pub fn parse_case_strict(line: &str) -> Result<CaseRequestStrict, ProtocolError> {
    check_line_size(line)?;
    serde_json::from_str::<CaseRequestStrict>(line).map_err(|e| ProtocolError::Json(e.to_string()))
}

// ---------------------------------------------------------------------------
// Target -> runner: ready / event / result / case_error
// ---------------------------------------------------------------------------

/// Target readiness reply: `{"protocol":"eval/v1","ready":true}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ready {
    pub protocol: String,
    pub ready: bool,
}

/// Progress / diagnostic, 0..n per case, ordered by `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub case_id: String,
    pub seq: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Event,
}

/// Artifact-bearing output. `files` are relative-path references the
/// runner records but does not fetch in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Output {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Optional runner-observed metrics. Absent means n/a, never zero-filled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Metrics {
    pub wall_ms: u64,
}

/// Optional AI usage block. Absent means n/a, never zero-filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AiUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Terminal success. The streamed events ARE the trace; there is no
/// duplicated trace array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResultMsg {
    #[serde(rename = "type")]
    pub kind: ResultKind,
    pub case_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Result,
}

/// Handled negative outcome (target behaved correctly, answer is "no").
/// Exit code stays 0; the runner grades it as a failure with `code`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CaseErrorMsg {
    #[serde(rename = "type")]
    pub kind: CaseErrorKind,
    pub case_id: String,
    pub code: String,
    pub message: String,
    pub retriable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaseErrorKind {
    CaseError,
}

/// Target -> runner inbound line (runner-side tolerant view).
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    Ready(Ready),
    Event(Event),
    Result(ResultMsg),
    CaseError(CaseErrorMsg),
}

// ---------------------------------------------------------------------------
// Encoding (runner -> target)
// ---------------------------------------------------------------------------

/// Encode handshake as first stdin line.
pub fn encode_handshake(mode: Mode) -> String {
    serde_json::to_string(&Handshake::new(mode)).expect("handshake serializes")
}

/// Encode a case request.
pub fn encode_case(msg: &CaseRequest) -> String {
    serde_json::to_string(msg).expect("case serializes")
}

/// Encode worker shutdown.
pub fn encode_shutdown() -> String {
    serde_json::to_string(&Shutdown {
        kind: ShutdownKind::Shutdown,
    })
    .expect("shutdown serializes")
}

/// Encode outbound helpers for targets (used by shims + conformance).
pub fn encode_event(msg: &Event) -> String {
    serde_json::to_string(msg).expect("event serializes")
}

pub fn encode_result(msg: &ResultMsg) -> String {
    serde_json::to_string(msg).expect("result serializes")
}

pub fn encode_case_error(msg: &CaseErrorMsg) -> String {
    serde_json::to_string(msg).expect("case_error serializes")
}

pub fn encode_ready() -> String {
    serde_json::to_string(&Ready {
        protocol: PROTOCOL_VERSION.to_string(),
        ready: true,
    })
    .expect("ready serializes")
}

// ---------------------------------------------------------------------------
// Decoding (runner reads target stdout)
// ---------------------------------------------------------------------------

fn check_line_size(line: &str) -> Result<(), ProtocolError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ProtocolError::LineTooLong {
            bytes: line.len(),
            max: MAX_LINE_BYTES,
        });
    }
    Ok(())
}

/// Parse one target stdout line (tolerant of unknown fields, strict
/// about shape). Anything else on stdout is a protocol violation —
/// callers must treat `Err` as `ProtocolError` (kill + respawn).
pub fn parse_line(line: &str) -> Result<Inbound, ProtocolError> {
    check_line_size(line)?;
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|e| ProtocolError::Json(e.to_string()))?;
    if !v.is_object() {
        return Err(ProtocolError::SchemaViolation(
            "top-level JSON must be an object".to_string(),
        ));
    }
    // Ready has no "type": {"protocol":"eval/v1","ready":true}.
    if v.get("ready").is_some() {
        let r: Ready =
            serde_json::from_value(v).map_err(|e| ProtocolError::SchemaViolation(e.to_string()))?;
        if r.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::VersionMismatch {
                expected: PROTOCOL_VERSION.to_string(),
                received: r.protocol,
            });
        }
        if !r.ready {
            return Err(ProtocolError::SchemaViolation(
                "ready must be true".to_string(),
            ));
        }
        return Ok(Inbound::Ready(r));
    }
    let t = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProtocolError::SchemaViolation("missing string field `type`".to_string()))?;
    match t {
        "event" => {
            let e: Event = serde_json::from_value(v)
                .map_err(|e| ProtocolError::SchemaViolation(e.to_string()))?;
            Ok(Inbound::Event(e))
        }
        "result" => {
            let r: ResultMsg = serde_json::from_value(v)
                .map_err(|e| ProtocolError::SchemaViolation(e.to_string()))?;
            Ok(Inbound::Result(r))
        }
        "case_error" => {
            let e: CaseErrorMsg = serde_json::from_value(v)
                .map_err(|e| ProtocolError::SchemaViolation(e.to_string()))?;
            Ok(Inbound::CaseError(e))
        }
        other => Err(ProtocolError::UnknownMessageType(other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Structural case validation at suite load.
pub fn validate_case(case: &CaseRequest) -> Vec<SchemaError> {
    let mut errs = Vec::new();
    if case.case_id.trim().is_empty() {
        errs.push(SchemaError {
            field: "case_id".to_string(),
            message: "must be non-empty".to_string(),
        });
    }
    if case.limits.timeout_ms < 1 {
        errs.push(SchemaError {
            field: "limits.timeout_ms".to_string(),
            message: "must be >= 1".to_string(),
        });
    }
    if case.limits.max_output_bytes < 1 {
        errs.push(SchemaError {
            field: "limits.max_output_bytes".to_string(),
            message: "must be >= 1".to_string(),
        });
    }
    errs
}

/// Timeout-rule check: runner deadline must be >= 1.5x the target's own
/// internal wall budget (where the target declares one).
pub fn satisfies_timeout_rule(runner_timeout_ms: u64, target_budget_ms: u64) -> bool {
    if target_budget_ms == 0 {
        return true;
    }
    (runner_timeout_ms as f64) >= (target_budget_ms as f64) * TIMEOUT_RULE_FACTOR
}

/// A single schema validation problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaError {
    pub field: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Ordering / correlation (runner-side session check)
// ---------------------------------------------------------------------------

/// Tracks per-case `seq` ordering. The runner rejects out-of-order or
/// cross-case lines as `ProtocolError`.
///
/// Usage: one tracker per worker connection; call [`SeqTracker::push_event`]
/// for each event. Terminal messages call [`SeqTracker::finish_case`].
#[derive(Debug, Default)]
pub struct SeqTracker {
    next_seq: HashMap<String, u64>,
}

impl SeqTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate an event's `seq` (0-based, gapless per case).
    pub fn push_event(&mut self, ev: &Event) -> Result<(), ProtocolError> {
        let next = self.next_seq.get(&ev.case_id).copied().unwrap_or(0);
        if ev.seq != next {
            return Err(ProtocolError::OutOfOrder {
                case_id: ev.case_id.clone(),
                expected_seq: next,
                received_seq: ev.seq,
            });
        }
        self.next_seq.insert(ev.case_id.clone(), next + 1);
        Ok(())
    }

    /// Mark a case finished (drops its seq cursor so a repeated case_id
    /// in a later run starts fresh; within one connection a duplicate
    /// terminal is left to the caller to reject).
    pub fn finish_case(&mut self, case_id: &str) {
        self.next_seq.remove(case_id);
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure taxonomy facet owned by the protocol layer. The full
/// taxonomy (ok / case_error / timeout / crash / protocol_error) lives
/// in `eval-runner` (M5); parsing failures here always map to
/// `protocol_error` there — never merged with handled `case_error`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
pub enum ProtocolError {
    #[error("protocol version mismatch: expected {expected}, received {received}")]
    VersionMismatch { expected: String, received: String },
    #[error("line too long: {bytes} bytes exceeds max {max}")]
    LineTooLong { bytes: usize, max: usize },
    #[error("invalid JSON: {0}")]
    Json(String),
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    #[error("unknown message type: {0}")]
    UnknownMessageType(String),
    #[error(
        "out-of-order event for case {case_id}: expected seq {expected_seq}, got {received_seq}"
    )]
    OutOfOrder {
        case_id: String,
        expected_seq: u64,
        received_seq: u64,
    },
}

// ---------------------------------------------------------------------------
// Schema generation (drift-checked in CI)
// ---------------------------------------------------------------------------

/// Combined schema document for drift checking. Additive changes are
/// minor; removing or retyping a required field is a major version
/// with a documented converter or explicit break notice.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProtocolSchemaDocument {
    pub protocol: String,
    pub handshake: schemars::Schema,
    pub handshake_strict: schemars::Schema,
    pub case: schemars::Schema,
    pub event: schemars::Schema,
    pub result: schemars::Schema,
    pub case_error: schemars::Schema,
    pub ready: schemars::Schema,
    pub shutdown: schemars::Schema,
}

/// Build the schema document written to
/// `protocol/eval-protocol-v1.schema.json`.
pub fn schema_document() -> ProtocolSchemaDocument {
    ProtocolSchemaDocument {
        protocol: PROTOCOL_VERSION.to_string(),
        handshake: schemars::schema_for!(Handshake),
        handshake_strict: schemars::schema_for!(HandshakeStrict),
        case: schemars::schema_for!(CaseRequest),
        event: schemars::schema_for!(Event),
        result: schemars::schema_for!(ResultMsg),
        case_error: schemars::schema_for!(CaseErrorMsg),
        ready: schemars::schema_for!(Ready),
        shutdown: schemars::schema_for!(Shutdown),
    }
}

/// Canonical pretty JSON for the schema file (2-space indent,
/// deterministic key order via `serde_json`). Ends with a trailing
/// newline so the committed artifact is a well-formed POSIX text file
/// and survives whitespace-normalizing hooks unchanged.
pub fn schema_json_pretty() -> String {
    let doc = schema_document();
    let mut json = serde_json::to_string_pretty(&doc).expect("schema serializes");
    json.push('\n');
    json
}
