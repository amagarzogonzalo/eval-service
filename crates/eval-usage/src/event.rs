use crate::error::UsageError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Contract version. Readers accept exactly this version; anything
/// else is rejected with a reason (no silent reinterpretation).
pub const EVENT_VERSION: u32 = 1;

/// Terminal outcome of one physical attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Error,
    Timeout,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Timeout => "timeout",
        }
    }
}

/// Where a cost figure came from. Estimated costs are never presented
/// as provider-reported billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CostProvenance {
    Reported,
    Estimated,
}

/// Where an event came from. Fixture data is labeled as such end to
/// end so it can never pass as measured production performance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventOrigin {
    #[default]
    Live,
    Replay,
    Synthetic,
}

/// What the measured call was part of. Production usage and
/// controlled benchmarks aggregate separately and must never mix:
/// a benchmark trial is not production traffic, and live traffic is
/// not a benchmark sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageContext {
    Production,
    Benchmark,
    Dev,
}

/// One LLM call. Field names reuse OpenTelemetry GenAI conventions
/// where they exist (`gen_ai.system` → `provider`,
/// `gen_ai.request.model` → `model`); everything else is this
/// contract's own versioned vocabulary.
///
/// Optional means unknown-or-inapplicable. Absent input_tokens does
/// NOT mean zero tokens — it means the integration could not observe
/// them (documented per integration).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmEvent {
    /// Always `"llm.call"`.
    pub event: String,
    /// Always [`EVENT_VERSION`].
    pub event_version: u32,
    /// Idempotency key. Re-ingesting the same id is a no-op duplicate.
    pub event_id: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Application-level task this call belongs to (one task may span
    /// many calls across retries, tools, and agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Stable logical request id; retries share it.
    pub request_id: String,
    /// Zero-based physical attempt number within the request.
    #[serde(default)]
    pub attempt: u32,
    pub provider: String,
    pub model: String,
    /// Unix millis when the request started.
    pub started_at_ms: u64,
    /// End-to-end duration in millis (measured caller-side).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default)]
    pub retriable: bool,
    /// Provider-reported cost. Never computed locally; see pricing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_provenance: Option<CostProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
    /// Association with a controlled evaluation run (benchmark joins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_id: Option<String>,
    /// Free-form benchmark/variant label (e.g. `bench-a`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Required. Production and benchmark traffic aggregate
    /// separately; dev/test traffic is labeled as such.
    pub context: UsageContext,
    #[serde(default)]
    pub origin: EventOrigin,
    /// Provider-specific extras under namespaced keys
    /// (e.g. `"groq.queue_time_ms"`). Never credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<HashMap<String, serde_json::Value>>,
}

/// Structural validation of one parsed event. Returns the event on
/// success; rejects unknown versions, empty ids, and negative-shaped
/// contradictions (e.g. attempt present without request context is
/// fine — attempt defaults to 0 — but empty project/provider are not).
pub fn validate_event(event: &LlmEvent) -> Result<(), UsageError> {
    if event.event_version != EVENT_VERSION {
        return Err(UsageError::UnsupportedVersion(event.event_version));
    }
    if event.event != "llm.call" {
        return Err(UsageError::Rejected {
            line: 0,
            reason: format!("unknown event type {:?}", event.event),
        });
    }
    for (field, value) in [
        ("event_id", event.event_id.as_str()),
        ("project", event.project.as_str()),
        ("request_id", event.request_id.as_str()),
        ("provider", event.provider.as_str()),
        ("model", event.model.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(UsageError::Rejected {
                line: 0,
                reason: format!("{field} must be non-empty"),
            });
        }
    }
    if event.started_at_ms == 0 {
        return Err(UsageError::Rejected {
            line: 0,
            reason: "started_at_ms must be non-zero".to_string(),
        });
    }
    if matches!(event.outcome, Outcome::Ok) && event.error_code.is_some() {
        return Err(UsageError::Rejected {
            line: 0,
            reason: "ok outcome must not carry error_code".to_string(),
        });
    }
    Ok(())
}
