//! Versioned run manifest (`run-manifest/v1`) and metrics (`run-metrics/v1`).
//!
//! The manifest describes one immutable run directory: identity, lane,
//! target/eval versions, budgets, observed metrics, evidence pointers,
//! and provenance. Metrics use `Option` end to end: `null` in JSON and
//! `n/a` in reports mean "not observed", never zero. Zero is valid only
//! when the measurement is genuinely zero.

use crate::config::TargetMode;
use crate::error::CoreError;
use crate::record::{CaseRecord, RunRecord, TrialStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub const RUN_MANIFEST_VERSION: u32 = 1;
pub const METRICS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Batch,
    LocalService,
    RemoteService,
    Replay,
}

impl Lane {
    pub const ALL: [Self; 4] = [
        Self::Batch,
        Self::LocalService,
        Self::RemoteService,
        Self::Replay,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Batch => "batch",
            Self::LocalService => "local-service",
            Self::RemoteService => "remote-service",
            Self::Replay => "replay",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Grader,
    CaseError,
    Timeout,
    Crash,
    ProtocolError,
    ServiceUnavailable,
    BudgetExceeded,
    MissingArtifact,
    Cancelled,
    Unknown,
}

impl FailureClass {
    pub const ALL: [Self; 10] = [
        Self::Grader,
        Self::CaseError,
        Self::Timeout,
        Self::Crash,
        Self::ProtocolError,
        Self::ServiceUnavailable,
        Self::BudgetExceeded,
        Self::MissingArtifact,
        Self::Cancelled,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grader => "grader",
            Self::CaseError => "case_error",
            Self::Timeout => "timeout",
            Self::Crash => "crash",
            Self::ProtocolError => "protocol_error",
            Self::ServiceUnavailable => "service_unavailable",
            Self::BudgetExceeded => "budget_exceeded",
            Self::MissingArtifact => "missing_artifact",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    Live,
    Replay,
    Synthetic,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostProvenance {
    Reported,
    Estimated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteRef {
    pub id: String,
    pub version: u32,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRef {
    pub name: Option<String>,
    pub version: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTimestamps {
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Budgets {
    pub target_budget_ms: Option<u64>,
    pub max_case_timeout_ms: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub cost_usd_cap: Option<f64>,
    pub tokens_cap: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: String,
    pub kind: Option<String>,
    pub bytes: Option<u64>,
    pub hash_alg: Option<String>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Paths {
    pub bucket: Option<String>,
    pub run_prefix: Option<String>,
    pub trace_key: Option<String>,
    pub artifact_prefix: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub trace_id: Option<String>,
    pub task_id: Option<String>,
    pub trace_url: Option<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub s3: Option<S3Paths>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub origin: RunOrigin,
    pub eval_git_commit: Option<String>,
    pub eval_git_dirty: Option<bool>,
    pub target_git_commit: Option<String>,
    pub host: Option<String>,
    pub runner: Option<String>,
    pub recorded_by: Option<String>,
    pub record_source: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseOutcomes {
    pub total: Option<u64>,
    pub passed: Option<u64>,
    pub failed: Option<u64>,
    pub infra: Option<u64>,
    pub timeouts: Option<u64>,
    pub skipped: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialCounts {
    pub planned: Option<u64>,
    pub completed: Option<u64>,
    pub usage_reporting: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyStats {
    pub n: u64,
    pub min_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

impl LatencyStats {
    pub fn from_samples(samples: &[u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank =
            |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
        Some(Self {
            n: sorted.len() as u64,
            min_ms: sorted[0],
            p50_ms: rank(0.5),
            p95_ms: rank(0.95),
            max_ms: sorted[sorted.len() - 1],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityStats {
    pub grader: String,
    pub mean_score: Option<f64>,
    pub n: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StabilityStats {
    pub passed: u64,
    pub total: u64,
}

impl StabilityStats {
    pub fn ratio(&self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some(self.passed as f64 / self.total as f64)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub model: Option<ModelRef>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_provenance: Option<CostProvenance>,
    pub pricing_version: Option<u32>,
    pub reporting_trials: Option<u64>,
    pub total_trials: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Availability {
    pub attempts: Option<u64>,
    pub successes: Option<u64>,
    pub ratio: Option<f64>,
}

impl Availability {
    pub fn observed_ratio(&self) -> Option<f64> {
        match (self.attempts, self.successes) {
            (Some(attempts), Some(successes)) if attempts > 0 => {
                Some(successes as f64 / attempts as f64)
            }
            _ => self.ratio,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceMetrics {
    pub availability: Option<Availability>,
    pub cold_start: Option<LatencyStats>,
    pub queue_wait: Option<LatencyStats>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMetrics {
    pub metrics_version: u32,
    pub trials: TrialCounts,
    pub outcomes: CaseOutcomes,
    pub failure_classes: BTreeMap<FailureClass, u64>,
    pub latency: Option<LatencyStats>,
    pub usage: Usage,
    pub quality: Option<QualityStats>,
    pub stability: Option<StabilityStats>,
    pub service: Option<ServiceMetrics>,
}

impl Default for RunMetrics {
    fn default() -> Self {
        Self {
            metrics_version: METRICS_SCHEMA_VERSION,
            trials: TrialCounts::default(),
            outcomes: CaseOutcomes::default(),
            failure_classes: BTreeMap::new(),
            latency: None,
            usage: Usage::default(),
            quality: None,
            stability: None,
            service: None,
        }
    }
}

impl RunMetrics {
    pub fn from_record(record: &RunRecord) -> Self {
        let mut metrics = Self::default();
        let mut walls = Vec::new();
        let mut failed = 0u64;
        let mut passed = 0u64;
        let mut infra = 0u64;
        let mut timeouts = 0u64;
        let mut completed = 0u64;
        let mut reporting = 0u64;
        let mut tokens_in = 0u64;
        let mut tokens_out = 0u64;
        let mut cost_usd = 0.0f64;
        let mut saw_in = false;
        let mut saw_out = false;
        let mut saw_cost = false;
        let mut judge_scores = Vec::new();
        let mut stability_passed = 0u64;
        let mut stability_total = 0u64;

        for case in &record.cases {
            match classify_case(case) {
                None => passed += 1,
                Some(class) => {
                    failed += 1;
                    *metrics.failure_classes.entry(class).or_insert(0) += 1;
                }
            }
            if !case.pass
                && case
                    .trials
                    .iter()
                    .any(|t| matches!(t.status, TrialStatus::Crash | TrialStatus::ProtocolError))
            {
                infra += 1;
            }
            if !case.pass && case.trials.iter().any(|t| t.status == TrialStatus::Timeout) {
                timeouts += 1;
            }
            for trial in &case.trials {
                completed += 1;
                walls.push(trial.wall_ms);
                if let Some(ai) = trial.output.as_ref().and_then(|o| o.get("ai")) {
                    let mut reported = false;
                    if let Some(v) = ai.get("tokens_in").and_then(|v| v.as_u64()) {
                        tokens_in += v;
                        saw_in = true;
                        reported = true;
                    }
                    if let Some(v) = ai.get("tokens_out").and_then(|v| v.as_u64()) {
                        tokens_out += v;
                        saw_out = true;
                        reported = true;
                    }
                    if let Some(v) = ai.get("cost_usd").and_then(|v| v.as_f64()) {
                        cost_usd += v;
                        saw_cost = true;
                        reported = true;
                    }
                    if reported {
                        reporting += 1;
                    }
                }
            }
            for grade in &case.grades {
                if grade.grader.starts_with("judge_lite") {
                    if let Some(score) = grade.score {
                        judge_scores.push(score);
                    }
                }
                if grade.grader == "stability" {
                    stability_total += 1;
                    if grade.pass {
                        stability_passed += 1;
                    }
                }
            }
        }

        metrics.outcomes = CaseOutcomes {
            total: Some(record.cases.len() as u64),
            passed: Some(passed),
            failed: Some(failed),
            infra: Some(infra),
            timeouts: Some(timeouts),
            skipped: None,
        };
        metrics.trials = TrialCounts {
            planned: None,
            completed: Some(completed),
            usage_reporting: Some(reporting),
        };
        metrics.latency = LatencyStats::from_samples(&walls);
        metrics.usage = Usage {
            model: None,
            tokens_in: saw_in.then_some(tokens_in),
            tokens_out: saw_out.then_some(tokens_out),
            cached_tokens: None,
            reasoning_tokens: None,
            cost_usd: saw_cost.then_some(cost_usd),
            cost_provenance: None,
            pricing_version: None,
            reporting_trials: Some(reporting),
            total_trials: Some(completed),
        };
        if !judge_scores.is_empty() {
            let n = judge_scores.len() as u64;
            metrics.quality = Some(QualityStats {
                grader: "judge_lite".to_string(),
                mean_score: Some(judge_scores.iter().sum::<f64>() / n as f64),
                n,
            });
        }
        if stability_total > 0 {
            metrics.stability = Some(StabilityStats {
                passed: stability_passed,
                total: stability_total,
            });
        }
        metrics
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.metrics_version != METRICS_SCHEMA_VERSION {
            return Err(CoreError::Config(format!(
                "run metrics version {} is not {METRICS_SCHEMA_VERSION}",
                self.metrics_version
            )));
        }
        if let Some(total) = self.outcomes.total {
            let counted = self.outcomes.passed.unwrap_or(0) + self.outcomes.failed.unwrap_or(0);
            if counted > total {
                return Err(CoreError::Config(format!(
                    "case outcomes exceed total: {counted} > {total}"
                )));
            }
        }
        if let Some(failed) = self.outcomes.failed {
            let classified: u64 = self.failure_classes.values().sum();
            if classified != failed {
                return Err(CoreError::Config(format!(
                    "failure classes cover {classified} cases but outcomes.failed is {failed}"
                )));
            }
            if let Some(timeouts) = self.outcomes.timeouts {
                if timeouts > failed {
                    return Err(CoreError::Config(format!(
                        "outcomes.timeouts ({timeouts}) exceeds failed ({failed})"
                    )));
                }
            }
        }
        if let Some(latency) = &self.latency {
            validate_latency("latency", latency)?;
        }
        if let Some(quality) = &self.quality {
            if quality.n == 0 {
                return Err(CoreError::Config(
                    "quality.n must be > 0 when quality is present".to_string(),
                ));
            }
            if let Some(mean) = quality.mean_score {
                if !(0.0..=1.0).contains(&mean) {
                    return Err(CoreError::Config(
                        "quality.mean_score must be in [0, 1]".to_string(),
                    ));
                }
            }
        }
        if let Some(stability) = &self.stability {
            if stability.total == 0 || stability.passed > stability.total {
                return Err(CoreError::Config(
                    "stability requires total > 0 and passed <= total".to_string(),
                ));
            }
        }
        if let (Some(reporting), Some(total)) =
            (self.usage.reporting_trials, self.usage.total_trials)
        {
            if reporting > total {
                return Err(CoreError::Config(
                    "usage.reporting_trials must not exceed usage.total_trials".to_string(),
                ));
            }
        }
        if let Some(cost) = self.usage.cost_usd {
            if !cost.is_finite() || cost < 0.0 {
                return Err(CoreError::Config(
                    "usage.cost_usd must be finite and >= 0".to_string(),
                ));
            }
        }
        if let Some(service) = &self.service {
            if let Some(availability) = &service.availability {
                if let Some(ratio) = availability.observed_ratio() {
                    if !(0.0..=1.0).contains(&ratio) {
                        return Err(CoreError::Config(
                            "service availability ratio must be in [0, 1]".to_string(),
                        ));
                    }
                }
                if let (Some(attempts), Some(successes)) =
                    (availability.attempts, availability.successes)
                {
                    if successes > attempts {
                        return Err(CoreError::Config(
                            "service successes must not exceed attempts".to_string(),
                        ));
                    }
                }
            }
            if let Some(cold) = &service.cold_start {
                validate_latency("service.cold_start", cold)?;
            }
            if let Some(queue) = &service.queue_wait {
                validate_latency("service.queue_wait", queue)?;
            }
        }
        Ok(())
    }
}

fn validate_latency(name: &str, latency: &LatencyStats) -> Result<(), CoreError> {
    if latency.n == 0 {
        return Err(CoreError::Config(format!("{name}.n must be > 0")));
    }
    if !(latency.min_ms <= latency.p50_ms
        && latency.p50_ms <= latency.p95_ms
        && latency.p95_ms <= latency.max_ms)
    {
        return Err(CoreError::Config(format!(
            "{name} percentiles are not monotonic"
        )));
    }
    Ok(())
}

pub fn classify_case(case: &CaseRecord) -> Option<FailureClass> {
    if case.pass {
        return None;
    }
    let has = |status: TrialStatus| case.trials.iter().any(|t| t.status == status);
    if has(TrialStatus::ProtocolError) {
        return Some(FailureClass::ProtocolError);
    }
    if has(TrialStatus::Crash) {
        return Some(FailureClass::Crash);
    }
    if has(TrialStatus::Timeout) {
        return Some(FailureClass::Timeout);
    }
    if has(TrialStatus::CaseError) {
        return Some(FailureClass::CaseError);
    }
    if case.grades.iter().any(|g| !g.pass) {
        return Some(FailureClass::Grader);
    }
    Some(FailureClass::Unknown)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunManifest {
    pub manifest_version: u32,
    pub run_id: String,
    pub suite: SuiteRef,
    pub target: TargetRef,
    pub lane: Option<Lane>,
    pub eval_version: Option<String>,
    pub ref_name: String,
    pub model: Option<ModelRef>,
    pub free_only: Option<bool>,
    pub timestamps: RunTimestamps,
    pub budgets: Budgets,
    pub metrics: RunMetrics,
    pub evidence: Evidence,
    pub provenance: Provenance,
}

impl RunManifest {
    pub fn from_record(
        record: &RunRecord,
        lane: Option<Lane>,
        eval_version: Option<String>,
        target_version: Option<String>,
        model: Option<ModelRef>,
        free_only: Option<bool>,
    ) -> Self {
        let mode = record.target.as_ref().map(|t| match t.mode {
            TargetMode::Worker => "worker".to_string(),
            TargetMode::Oneshot => "oneshot".to_string(),
        });
        let mut metrics = RunMetrics::from_record(record);
        metrics.usage.model = model.clone();
        Self {
            manifest_version: RUN_MANIFEST_VERSION,
            run_id: record.run_id.clone(),
            suite: SuiteRef {
                id: record.suite.clone(),
                version: record.suite_version,
                path: None,
            },
            target: TargetRef {
                name: None,
                version: target_version,
                mode,
            },
            lane,
            eval_version,
            ref_name: record.ref_name.clone(),
            model,
            free_only,
            timestamps: RunTimestamps {
                started_at_ms: record.started_at_ms,
                finished_at_ms: None,
                duration_ms: None,
            },
            budgets: Budgets {
                target_budget_ms: record.target.as_ref().and_then(|t| t.budget_ms),
                ..Default::default()
            },
            metrics,
            evidence: Evidence::default(),
            provenance: Provenance::default(),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.manifest_version != RUN_MANIFEST_VERSION {
            return Err(CoreError::Config(format!(
                "run manifest version {} is not {RUN_MANIFEST_VERSION}",
                self.manifest_version
            )));
        }
        if self.run_id.trim().is_empty() {
            return Err(CoreError::Config(
                "run manifest requires a non-empty run_id".to_string(),
            ));
        }
        if self.suite.id.trim().is_empty() {
            return Err(CoreError::Config(
                "run manifest requires a non-empty suite id".to_string(),
            ));
        }
        if self.ref_name.trim().is_empty() {
            return Err(CoreError::Config(
                "run manifest requires a non-empty ref_name".to_string(),
            ));
        }
        if let Some(finished) = self.timestamps.finished_at_ms {
            if finished < self.timestamps.started_at_ms {
                return Err(CoreError::Config(
                    "timestamps.finished_at_ms precedes started_at_ms".to_string(),
                ));
            }
        }
        if let Some(cap) = self.budgets.cost_usd_cap {
            if !cap.is_finite() || cap < 0.0 {
                return Err(CoreError::Config(
                    "budgets.cost_usd_cap must be finite and >= 0".to_string(),
                ));
            }
        }
        for artifact in &self.evidence.artifacts {
            if artifact.path.trim().is_empty() {
                return Err(CoreError::Config(
                    "artifact paths must be non-empty".to_string(),
                ));
            }
            if artifact.hash.is_some() && artifact.hash_alg.is_none() {
                return Err(CoreError::Config(format!(
                    "artifact {} has a hash but no hash_alg",
                    artifact.path
                )));
            }
        }
        self.metrics.validate()
    }

    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(path.display().to_string(), e))?;
        let manifest: Self =
            serde_json::from_str(&text).map_err(|e| CoreError::Json(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_json_pretty(&self) -> Result<String, CoreError> {
        let mut json =
            serde_json::to_string_pretty(self).map_err(|e| CoreError::Json(e.to_string()))?;
        json.push('\n');
        Ok(json)
    }
}

fn nullable(kind: &str) -> Value {
    json!({"type": [kind, "null"]})
}

fn nullable_count() -> Value {
    json!({"type": ["integer", "null"], "minimum": 0})
}

fn latency_schema() -> Value {
    json!({
        "type": "object",
        "required": ["n", "min_ms", "p50_ms", "p95_ms", "max_ms"],
        "properties": {
            "n": {"type": "integer", "minimum": 1},
            "min_ms": {"type": "integer", "minimum": 0},
            "p50_ms": {"type": "integer", "minimum": 0},
            "p95_ms": {"type": "integer", "minimum": 0},
            "max_ms": {"type": "integer", "minimum": 0}
        }
    })
}

fn lane_enum() -> Value {
    let mut values: Vec<Value> = Lane::ALL.iter().map(|lane| json!(lane.as_str())).collect();
    values.push(Value::Null);
    json!({"type": ["string", "null"], "enum": values})
}

pub fn manifest_json_schema() -> Value {
    let mut failure_names: Vec<&str> = FailureClass::ALL
        .iter()
        .map(|class| class.as_str())
        .collect();
    failure_names.sort_unstable();
    let failure_values: Vec<Value> = failure_names.iter().map(|name| json!(name)).collect();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "urn:eval-service:run-manifest:v1",
        "title": "run-manifest/v1",
        "type": "object",
        "required": [
            "manifest_version", "run_id", "suite", "target", "ref_name",
            "timestamps", "budgets", "metrics", "evidence", "provenance"
        ],
        "properties": {
            "manifest_version": {"const": RUN_MANIFEST_VERSION},
            "run_id": {"type": "string", "minLength": 1},
            "suite": {
                "type": "object",
                "required": ["id", "version", "path"],
                "properties": {
                    "id": {"type": "string", "minLength": 1},
                    "version": {"type": "integer", "minimum": 0},
                    "path": nullable("string")
                }
            },
            "target": {
                "type": "object",
                "required": ["name", "version", "mode"],
                "properties": {
                    "name": nullable("string"),
                    "version": nullable("string"),
                    "mode": {
                        "type": ["string", "null"],
                        "enum": ["worker", "oneshot", null]
                    }
                }
            },
            "lane": lane_enum(),
            "eval_version": nullable("string"),
            "ref_name": {"type": "string", "minLength": 1},
            "model": {
                "type": ["object", "null"],
                "required": ["provider", "model"],
                "properties": {
                    "provider": nullable("string"),
                    "model": {"type": "string", "minLength": 1}
                }
            },
            "free_only": nullable("boolean"),
            "timestamps": {
                "type": "object",
                "required": ["started_at_ms", "finished_at_ms", "duration_ms"],
                "properties": {
                    "started_at_ms": {"type": "integer", "minimum": 0},
                    "finished_at_ms": nullable_count(),
                    "duration_ms": nullable_count()
                }
            },
            "budgets": {
                "type": "object",
                "required": [
                    "target_budget_ms", "max_case_timeout_ms", "max_output_bytes",
                    "cost_usd_cap", "tokens_cap"
                ],
                "properties": {
                    "target_budget_ms": nullable_count(),
                    "max_case_timeout_ms": nullable_count(),
                    "max_output_bytes": nullable_count(),
                    "cost_usd_cap": {
                        "type": ["number", "null"],
                        "minimum": 0
                    },
                    "tokens_cap": nullable_count()
                }
            },
            "metrics": {
                "type": "object",
                "required": [
                    "metrics_version", "trials", "outcomes", "failure_classes",
                    "latency", "usage", "quality", "stability", "service"
                ],
                "properties": {
                    "metrics_version": {"const": METRICS_SCHEMA_VERSION},
                    "trials": {
                        "type": "object",
                        "required": ["planned", "completed", "usage_reporting"],
                        "properties": {
                            "planned": nullable_count(),
                            "completed": nullable_count(),
                            "usage_reporting": nullable_count()
                        }
                    },
                    "outcomes": {
                        "type": "object",
                        "required": [
                            "total", "passed", "failed", "infra", "timeouts", "skipped"
                        ],
                        "properties": {
                            "total": nullable_count(),
                            "passed": nullable_count(),
                            "failed": nullable_count(),
                            "infra": nullable_count(),
                            "timeouts": nullable_count(),
                            "skipped": nullable_count()
                        }
                    },
                    "failure_classes": {
                        "type": "object",
                        "propertyNames": {"enum": failure_values},
                        "additionalProperties": {"type": "integer", "minimum": 0}
                    },
                    "latency": {
                        "oneOf": [latency_schema(), {"type": "null"}]
                    },
                    "usage": {
                        "type": "object",
                        "required": [
                            "model", "tokens_in", "tokens_out", "cached_tokens",
                            "reasoning_tokens", "cost_usd", "cost_provenance",
                            "pricing_version", "reporting_trials", "total_trials"
                        ],
                        "properties": {
                            "model": {
                                "type": ["object", "null"],
                                "required": ["provider", "model"],
                                "properties": {
                                    "provider": nullable("string"),
                                    "model": {"type": "string", "minLength": 1}
                                }
                            },
                            "tokens_in": nullable_count(),
                            "tokens_out": nullable_count(),
                            "cached_tokens": nullable_count(),
                            "reasoning_tokens": nullable_count(),
                            "cost_usd": {
                                "type": ["number", "null"],
                                "minimum": 0
                            },
                            "cost_provenance": {
                                "type": ["string", "null"],
                                "enum": ["reported", "estimated", null]
                            },
                            "pricing_version": nullable_count(),
                            "reporting_trials": nullable_count(),
                            "total_trials": nullable_count()
                        }
                    },
                    "quality": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["grader", "mean_score", "n"],
                                "properties": {
                                    "grader": {"type": "string", "minLength": 1},
                                    "mean_score": {
                                        "type": ["number", "null"],
                                        "minimum": 0,
                                        "maximum": 1
                                    },
                                    "n": {"type": "integer", "minimum": 1}
                                }
                            },
                            {"type": "null"}
                        ]
                    },
                    "stability": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["passed", "total"],
                                "properties": {
                                    "passed": {"type": "integer", "minimum": 0},
                                    "total": {"type": "integer", "minimum": 1}
                                }
                            },
                            {"type": "null"}
                        ]
                    },
                    "service": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["availability", "cold_start", "queue_wait"],
                                "properties": {
                                    "availability": {
                                        "oneOf": [
                                            {
                                                "type": "object",
                                                "required": ["attempts", "successes", "ratio"],
                                                "properties": {
                                                    "attempts": nullable_count(),
                                                    "successes": nullable_count(),
                                                    "ratio": {
                                                        "type": ["number", "null"],
                                                        "minimum": 0,
                                                        "maximum": 1
                                                    }
                                                }
                                            },
                                            {"type": "null"}
                                        ]
                                    },
                                    "cold_start": {
                                        "oneOf": [latency_schema(), {"type": "null"}]
                                    },
                                    "queue_wait": {
                                        "oneOf": [latency_schema(), {"type": "null"}]
                                    }
                                }
                            },
                            {"type": "null"}
                        ]
                    }
                }
            },
            "evidence": {
                "type": "object",
                "required": ["trace_id", "task_id", "trace_url", "artifacts", "s3"],
                "properties": {
                    "trace_id": nullable("string"),
                    "task_id": nullable("string"),
                    "trace_url": nullable("string"),
                    "artifacts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["path", "kind", "bytes", "hash_alg", "hash"],
                            "properties": {
                                "path": {"type": "string", "minLength": 1},
                                "kind": nullable("string"),
                                "bytes": nullable_count(),
                                "hash_alg": nullable("string"),
                                "hash": nullable("string")
                            }
                        }
                    },
                    "s3": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": [
                                    "bucket", "run_prefix", "trace_key", "artifact_prefix"
                                ],
                                "properties": {
                                    "bucket": nullable("string"),
                                    "run_prefix": nullable("string"),
                                    "trace_key": nullable("string"),
                                    "artifact_prefix": nullable("string")
                                }
                            },
                            {"type": "null"}
                        ]
                    }
                }
            },
            "provenance": {
                "type": "object",
                "required": [
                    "origin", "eval_git_commit", "eval_git_dirty",
                    "target_git_commit", "host", "runner", "recorded_by",
                    "record_source", "notes"
                ],
                "properties": {
                    "origin": {
                        "type": "string",
                        "enum": ["live", "replay", "synthetic", "unknown"]
                    },
                    "eval_git_commit": nullable("string"),
                    "eval_git_dirty": nullable("boolean"),
                    "target_git_commit": nullable("string"),
                    "host": nullable("string"),
                    "runner": nullable("string"),
                    "recorded_by": nullable("string"),
                    "record_source": nullable("string"),
                    "notes": nullable("string")
                }
            }
        }
    })
}
