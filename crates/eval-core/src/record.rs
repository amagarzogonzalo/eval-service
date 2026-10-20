use crate::config::{CaseSpec, TargetSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stored-run schema version. Migrations are forward-only converters
/// applied on read; old runs are never rewritten.
pub const SCHEMA_VERSION: u32 = 1;

/// Timeout-rule factor shared with the protocol spec: runner deadlines
/// must be >= 1.5x the target's declared internal budget.
pub const TIMEOUT_RULE_FACTOR: f64 = 1.5;

/// Terminal status of one trial. `CaseError` is a handled negative
/// outcome; the other three are infrastructure observations. Never merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialStatus {
    Ok,
    CaseError,
    Timeout,
    Crash,
    ProtocolError,
}

impl TrialStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::CaseError => "case_error",
            Self::Timeout => "timeout",
            Self::Crash => "crash",
            Self::ProtocolError => "protocol_error",
        }
    }

    /// `true` for infrastructure failures (exit code 2 territory).
    pub fn is_infra(&self) -> bool {
        matches!(self, Self::Timeout | Self::Crash | Self::ProtocolError)
    }
}

/// Raw observation of one trial (immutable once written).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrialObservation {
    pub case_id: String,
    pub trial: u32,
    pub status: TrialStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Runner-measured wall time. Always present, never zero-filled.
    pub wall_ms: u64,
    /// Terminal message verbatim (`result` or `case_error`), redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    /// Streamed events verbatim, redacted.
    #[serde(default)]
    pub events: Vec<Value>,
    /// Capped + redacted stderr tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// One derived grade (recomputable from observations).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    pub grader: String,
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub evidence: String,
}

/// All trials + grades for one case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseRecord {
    pub case_id: String,
    pub deterministic: bool,
    pub trials: Vec<TrialObservation>,
    pub grades: Vec<Grade>,
    pub pass: bool,
    /// Stable content digest (excludes wall-clock noise).
    pub digest: String,
}

/// Suite-gate decision with a written explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDecision {
    pub pass: bool,
    pub what_changed: String,
    #[serde(default)]
    pub affected_cases: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<String>,
}

/// One stored run: suite + versions + trials + grades + gate decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u32,
    pub run_id: String,
    pub suite: String,
    pub suite_version: u32,
    pub code_version: String,
    pub ref_name: String,
    pub started_at_ms: u64,
    /// Optional caller-supplied idempotency key (e.g. a watch request).
    /// Additive, omitted from old records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub target: Option<TargetSpec>,
    pub files_root: String,
    pub trials_total: usize,
    pub cases: Vec<CaseRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateDecision>,
}

impl RunRecord {
    /// Overall pass: every case passed.
    pub fn pass(&self) -> bool {
        self.cases.iter().all(|c| c.pass)
    }

    /// `true` if a *failing* case observed a crash or protocol
    /// violation. A deliberate nonzero exit that passes its
    /// `exit_code` grader (the conventional-CLI rejection pattern) is
    /// a graded pass, not an infrastructure failure.
    pub fn has_infra_failure(&self) -> bool {
        self.cases.iter().any(|c| {
            !c.pass
                && c.trials
                    .iter()
                    .any(|t| matches!(t.status, TrialStatus::Crash | TrialStatus::ProtocolError))
        })
    }

    /// Mean case pass rate in [0, 1].
    pub fn pass_rate(&self) -> f64 {
        if self.cases.is_empty() {
            return 0.0;
        }
        let passed = self.cases.iter().filter(|c| c.pass).count() as f64;
        passed / self.cases.len() as f64
    }

    /// Code version: explicit override wins, else `dirty`.
    pub fn code_version() -> String {
        std::env::var("EVAL_CODE_VERSION")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "dirty".to_string())
    }
}

/// Normalized trial value for stable digests: drops wall-clock noise
/// (`metrics.wall_ms`), keeps everything semantic.
pub fn normalize_trial_value(trial: &TrialObservation) -> Value {
    let mut output = trial.output.clone().unwrap_or(Value::Null);
    // Nested (not let-chains): MSRV 1.85.
    if let Value::Object(map) = &mut output {
        if let Some(Value::Object(metrics)) = map.get_mut("metrics") {
            metrics.remove("wall_ms");
            if metrics.is_empty() {
                map.remove("metrics");
            }
        }
    }
    serde_json::json!({
        "case_id": trial.case_id,
        "status": trial.status.as_str(),
        "exit_code": trial.exit_code,
        "output": output,
        "events": trial.events,
        "error_code": trial.error_code,
        "error_message": trial.error_message,
    })
}

/// Stable digest over a case's trials (excludes wall-clock noise).
pub fn digest_trials(trials: &[TrialObservation]) -> String {
    let normalized: Vec<Value> = trials.iter().map(normalize_trial_value).collect();
    let bytes = serde_json::to_vec(&normalized).unwrap_or_default();
    crate::ids::hash8(&bytes)
}

/// Digest over a full run (sorted case digests + grades).
pub fn digest_run(cases: &[CaseRecord]) -> String {
    let mut parts: Vec<String> = cases
        .iter()
        .map(|c| {
            let grade_bits: Vec<String> = c
                .grades
                .iter()
                .map(|g| format!("{}={}", g.grader, g.pass as u8))
                .collect();
            format!("{}:{}:[{}]", c.case_id, c.digest, grade_bits.join(","))
        })
        .collect();
    parts.sort();
    crate::ids::hash8(parts.join("\n").as_bytes())
}

/// Convenience: build case records from specs + observations + grades.
pub fn assemble_case(
    spec: &CaseSpec,
    trials: Vec<TrialObservation>,
    grades: Vec<Grade>,
) -> CaseRecord {
    let pass = !grades.is_empty() && grades.iter().all(|g| g.pass);
    let digest = digest_trials(&trials);
    CaseRecord {
        case_id: spec.id.clone(),
        deterministic: spec.deterministic,
        trials,
        grades,
        pass,
        digest,
    }
}
