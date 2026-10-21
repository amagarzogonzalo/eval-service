#![recursion_limit = "256"]

//! `eval-core`: sync domain, config, run records, file store, redaction.
//!
//! Depends on nothing in the workspace. Only this crate defines what a
//! suite, a trial, a grade, and a gate decision are.

pub mod config;
pub mod error;
pub mod ids;
pub mod record;
pub mod redact;
pub mod run_manifest;
pub mod store;
pub mod workspace;

pub use config::{
    CaseSpec, GatePolicy, GraderSpec, InputSpec, LimitsSpec, PRESET_NAMES, PresetSpec, Suite,
    TargetMode, TargetSpec, resolve_preset,
};
pub use error::CoreError;
pub use ids::{hash8, new_run_id, now_ms};
pub use record::{
    CaseRecord, GateDecision, Grade, RunRecord, SCHEMA_VERSION, TrialObservation, TrialStatus,
    digest_run, digest_trials, normalize_trial_value,
};
pub use redact::{REDACTED, redact_json, redact_string};
pub use run_manifest::{
    ArtifactRef, Availability, Budgets, CaseOutcomes, CostProvenance, Evidence, FailureClass, Lane,
    LatencyStats, METRICS_SCHEMA_VERSION, ModelRef, Provenance, QualityStats, RUN_MANIFEST_VERSION,
    RunManifest, RunMetrics, RunOrigin, RunTimestamps, S3Paths, ServiceMetrics, StabilityStats,
    SuiteRef, TargetRef, TrialCounts, Usage, classify_case, manifest_json_schema,
};
pub use store::{BaselinePointer, ManifestBackfillReport, RunStore};
pub use workspace::{ProjectEntry, WorkspaceFile, WorkspaceMeta};
