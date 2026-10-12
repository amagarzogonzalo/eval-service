//! `eval-usage`: versioned LLM-call events, idempotent file store,
//! versioned pricing, and aggregation.
//!
//! One record per model call. Unknown fields are tolerated on read
//! (forward compatibility); unknown event versions are rejected with
//! a reason. Absent metrics stay absent — never zero-filled, never
//! fabricated.

pub mod aggregate;
pub mod collector;
pub mod error;
pub mod event;
pub mod pricing;
pub mod store;

pub use aggregate::{
    LatencyDist, ModelRow, OptSum, ProjectUsage, TaskRollup, TimeBucket, aggregate_project,
    aggregate_tasks, bucketize_daily,
};
pub use collector::{CollectorConfig, MAX_BODY_BYTES, serve};
pub use error::UsageError;
pub use event::{
    CostProvenance, EVENT_VERSION, EventOrigin, LlmEvent, Outcome, UsageContext, validate_event,
};
pub use pricing::{CostEstimate, PriceRate, PriceTable, Provenance};
pub use store::{ImportSummary, Rejection, TagBinding, UsageStore};
