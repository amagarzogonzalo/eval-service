//! `eval-graders`: deterministic graders, hand-rolled statistics,
//! baseline comparison, and the suite-gate decision.
//!
//! Statistics are deliberately dependency-free (three small, exactly
//! tested functions). No heuristic threshold is ever presented as
//! significance: every gate failure carries evidence lines.

pub mod compare;
pub mod dashboard;
pub mod fields;
pub mod graders;
pub mod portfolio;
pub mod stats;

pub use compare::decide_gate;
pub use dashboard::{esc, render_portfolio};
pub use fields::extract_field;
pub use graders::{check_schema, grade_case, grade_one, judge_lite_score};
pub use portfolio::{
    FailedCase, HistoryEntry, LatencyStats, PortfolioReport, PortfolioTotals, ProjectReport,
    ProjectSuiteReport, SuiteRefReport, TokenTotals, build_portfolio,
};
pub use stats::{bootstrap_paired_delta, pass_at_k, wilson_interval};
