//! Cross-project portfolio aggregation.
//!
//! Reads the canonical run stores through `RunStore` — never a shadow
//! database. Unavailable projects, missing suites, and corrupt records
//! are reported as first-class entries, never silently dropped.
//! Absent metrics stay `None` (rendered `n/a`), never zero-filled.

use eval_core::run_manifest::{RunManifest, RunMetrics};
use eval_core::{GatePolicy, RunRecord, RunStore, Suite, WorkspaceFile};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How many recent runs per suite ref feed history lists.
pub const HISTORY_PER_REF: usize = 5;
/// Cap on project-level merged history.
pub const HISTORY_PER_PROJECT: usize = 20;
/// How many recent runs per suite feed the trend view.
pub const TREND_LIMIT: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioReport {
    pub workspace_name: String,
    pub generated_at_ms: u64,
    pub projects: Vec<ProjectReport>,
    pub totals: PortfolioTotals,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PortfolioTotals {
    pub projects_total: usize,
    pub projects_available: usize,
    pub suites_total: usize,
    pub suites_with_runs: usize,
    pub cases_total: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    pub cases_infra: usize,
    /// Honest definition of the headline rate (case-weighted, not magic).
    pub pass_rate_note: String,
}

impl PortfolioTotals {
    pub fn pass_rate(&self) -> Option<f64> {
        if self.cases_total == 0 {
            None
        } else {
            Some(self.cases_passed as f64 / self.cases_total as f64)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectReport {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailability: Option<String>,
    #[serde(default)]
    pub suites: Vec<ProjectSuiteReport>,
    pub cases_total: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    pub cases_infra: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_ms: Option<u64>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// Measured usage (llm/v1 events). `None` until attached via
    /// [`attach_usage`]; legacy `ai` blocks never land here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageView>,
}

/// Measured LLM usage attached to a project report.
///
/// Precedence rule (never double-count): usage views aggregate
/// `llm/v1` events ONLY. Legacy result-attached `ai` blocks (the old
/// `AiUsage` result fields) remain visible inside run views, flagged
/// by `has_legacy_ai`, and are never added to event totals. The two
/// describe different things — per-call measurements vs. aggregate
/// result annotations — and summing them would count usage twice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production: Option<eval_usage::ProjectUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<eval_usage::ProjectUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<eval_usage::ProjectUsage>,
    pub has_legacy_ai: bool,
    /// Per-tag benchmark comparisons (usage + linked grade quality).
    #[serde(default)]
    pub comparisons: Vec<TagCompare>,
}

/// One benchmark variant row: measured usage joined with graded
/// quality through the tag binding. Origins and pricing versions are
/// part of the row so fixture demos can never read as model evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCompare {
    pub tag: String,
    pub suite: String,
    pub ref_name: String,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,
    pub quality_n: usize,
    pub calls: usize,
    pub input_tokens: eval_usage::OptSum,
    pub output_tokens: eval_usage::OptSum,
    pub estimated_usd: eval_usage::OptSum,
    pub pricing_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<eval_usage::LatencyDist>,
    pub origins: Vec<String>,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSuiteReport {
    pub suite_id: String,
    pub suite_path: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub refs: Vec<SuiteRefReport>,
    /// Newest-first, across refs.
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// Newest-first measurement points for trend review.
    #[serde(default)]
    pub trend: Vec<TrendPoint>,
    /// Fresh recomputation against the baseline pointer, when possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regression: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regression_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteRefReport {
    pub ref_name: String,
    pub run_id: String,
    pub started_at_ms: u64,
    pub code_version: String,
    pub pass: bool,
    pub pass_rate: f64,
    pub cases_total: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    pub cases_infra: usize,
    pub trials_total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyStats>,
    pub tokens: TokenTotals,
    #[serde(default)]
    pub failed_cases: Vec<FailedCase>,
    #[serde(default)]
    pub failure_classes: Vec<FailureClassCount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<RunQuality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability: Option<RunStability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<RunManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    /// Absolute run directory (for artifact links).
    pub run_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureClassCount {
    pub class: String,
    pub cases: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RunQuality {
    pub mean_score: f64,
    pub n: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RunStability {
    pub passed: u64,
    pub total: u64,
}

impl RunStability {
    pub fn ratio(&self) -> Option<f64> {
        if self.total == 0 {
            None
        } else {
            Some(self.passed as f64 / self.total as f64)
        }
    }
}

impl FailureClassCount {
    pub fn label(&self) -> &str {
        self.class.as_str()
    }
}

pub fn failure_counts(metrics: &RunMetrics) -> Vec<FailureClassCount> {
    let mut rows: Vec<FailureClassCount> = metrics
        .failure_classes
        .iter()
        .filter(|(_, cases)| **cases > 0)
        .map(|(class, cases)| FailureClassCount {
            class: class.as_str().to_string(),
            cases: *cases,
        })
        .collect();
    rows.sort_by(|a, b| b.cases.cmp(&a.cases).then_with(|| a.class.cmp(&b.class)));
    rows
}

pub fn quality_view(metrics: &RunMetrics) -> Option<RunQuality> {
    metrics.quality.as_ref().and_then(|q| {
        q.mean_score
            .map(|mean_score| RunQuality { mean_score, n: q.n })
    })
}

pub fn stability_view(metrics: &RunMetrics) -> Option<RunStability> {
    metrics.stability.as_ref().map(|s| RunStability {
        passed: s.passed,
        total: s.total,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyStats {
    pub n: usize,
    pub min_ms: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenTotals {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub reporting_trials: usize,
    pub total_trials: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedCase {
    pub case_id: String,
    pub statuses: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub suite: String,
    pub ref_name: String,
    pub run_id: String,
    pub started_at_ms: u64,
    pub pass: bool,
    pub pass_rate: f64,
    pub code_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub suite: String,
    pub ref_name: String,
    pub run_id: String,
    pub started_at_ms: u64,
    pub pass: bool,
    pub pass_rate: f64,
    pub cases_total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyStats>,
    pub tokens: TokenTotals,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
}

/// Build the portfolio by reading every registered project's store.
pub fn build_portfolio(workspace_path: &Path, file: &WorkspaceFile) -> PortfolioReport {
    let anchor = eval_core::workspace::WorkspaceFile::anchor(workspace_path);
    let mut projects = Vec::new();
    for entry in &file.project {
        projects.push(build_project(&anchor, entry));
    }
    let mut totals = PortfolioTotals {
        projects_total: projects.len(),
        projects_available: projects.iter().filter(|p| p.available).count(),
        pass_rate_note: "Overall rate = passed cases / total cases across available projects. \
            It weights projects by case count, not by importance; compare per-project rates \
            for like-with-like judgments."
            .to_string(),
        ..Default::default()
    };
    for project in &projects {
        totals.suites_total += project.suites.len();
        for suite in &project.suites {
            let has_runs = suite.refs.iter().any(|r| r.load_error.is_none());
            if has_runs {
                totals.suites_with_runs += 1;
            }
            // Totals reflect the newest run per suite only: summing every
            // ref would double-count cases across baseline/work histories.
            if let Some(newest) = newest_ref(&suite.refs) {
                totals.cases_total += newest.cases_total;
                totals.cases_passed += newest.cases_passed;
                totals.cases_failed += newest.cases_failed;
                totals.cases_infra += newest.cases_infra;
            }
        }
    }
    PortfolioReport {
        workspace_name: file.workspace.name.clone(),
        generated_at_ms: eval_core::now_ms(),
        projects,
        totals,
    }
}

/// Attach measured usage (llm/v1 events) to every available project.
///
/// Reads each project's usage store (`<store>/usage/<project-id>/`)
/// and aggregates per context. Legacy result-attached `ai` blocks are
/// detected (`has_legacy_ai`) but never merged into event totals —
/// see [`UsageView`]. Pricing resolves from `<store>/pricing.toml`
/// when present, else costs stay unknown.
pub fn attach_usage(
    workspace_path: &Path,
    file: &eval_core::WorkspaceFile,
    report: &mut PortfolioReport,
) {
    use eval_usage::{UsageContext, UsageStore, aggregate_project};

    let anchor = eval_core::workspace::WorkspaceFile::anchor(workspace_path);
    let by_id: std::collections::HashMap<&str, &eval_core::workspace::ProjectEntry> =
        file.project.iter().map(|e| (e.id.as_str(), e)).collect();
    for project in &mut report.projects {
        let Some(entry) = by_id.get(project.id.as_str()) else {
            continue;
        };
        if !project.available {
            continue;
        }
        let store_dir = entry.store_dir(&anchor);
        let pricing_path = store_dir.join("pricing.toml");
        let pricing = if pricing_path.exists() {
            eval_usage::PriceTable::load(&pricing_path)
                .unwrap_or_else(|_| eval_usage::PriceTable::empty())
        } else {
            eval_usage::PriceTable::empty()
        };
        let usage_store = UsageStore::new(&store_dir);
        let events = usage_store
            .read_events(&project.id)
            .map(|(events, _)| events)
            .unwrap_or_default();
        let legacy = project.suites.iter().any(|s| {
            s.refs
                .iter()
                .any(|r| r.load_error.is_none() && r.tokens.reporting_trials > 0)
        });
        let mut view = UsageView {
            has_legacy_ai: legacy,
            ..Default::default()
        };
        if !events.is_empty() {
            view.production = Some(aggregate_project(
                &project.id,
                &events,
                &pricing,
                Some(UsageContext::Production),
            ));
            view.benchmark = Some(aggregate_project(
                &project.id,
                &events,
                &pricing,
                Some(UsageContext::Benchmark),
            ));
            view.dev = Some(aggregate_project(
                &project.id,
                &events,
                &pricing,
                Some(UsageContext::Dev),
            ));
            view.comparisons = build_comparisons(&store_dir, &project.id, &events, &pricing);
        }
        project.usage = Some(view);
    }
}

/// Per-tag benchmark comparison rows. Usage aggregates filter by tag;
/// quality is the mean `judge_lite` grade score of the bound run (None
/// when the run is gone or never graded quality). Origins travel with
/// every row so synthetic demos read as demos.
fn build_comparisons(
    store_dir: &std::path::Path,
    project_id: &str,
    events: &[eval_usage::LlmEvent],
    pricing: &eval_usage::PriceTable,
) -> Vec<TagCompare> {
    use eval_usage::aggregate_project;

    let usage_store = eval_usage::UsageStore::new(store_dir);
    let bindings = usage_store.tag_bindings(project_id).unwrap_or_default();
    let runs = eval_core::RunStore::new(store_dir);
    let mut rows = Vec::new();
    let mut tags: Vec<&String> = bindings.keys().collect();
    tags.sort();
    for tag in tags {
        let binding = &bindings[tag.as_str()];
        let tagged: Vec<eval_usage::LlmEvent> = events
            .iter()
            .filter(|e| e.tag.as_deref() == Some(tag.as_str()))
            .cloned()
            .collect();
        // Usage side aggregates all tagged traffic (benchmark
        // procedure tags benchmark traffic; the row shows origins).
        let agg = aggregate_project(project_id, &tagged, pricing, None);
        let mut origins: Vec<String> = agg.by_model.iter().map(|r| r.origin.clone()).collect();
        origins.sort();
        origins.dedup();
        // Quality side: mean judge_lite score of the bound run.
        let (quality, quality_n) =
            match runs.load(&binding.suite, &binding.ref_name, &binding.run_id) {
                Ok(record) => {
                    let mut scores = Vec::new();
                    for case in &record.cases {
                        for grade in &case.grades {
                            if grade.grader.starts_with("judge_lite") {
                                if let Some(score) = grade.score {
                                    scores.push(score);
                                }
                            }
                        }
                    }
                    let n = scores.len();
                    let mean = if n == 0 {
                        None
                    } else {
                        Some(scores.iter().sum::<f64>() / n as f64)
                    };
                    (mean, n)
                }
                Err(_) => (None, 0),
            };
        rows.push(TagCompare {
            tag: tag.to_string(),
            suite: binding.suite.clone(),
            ref_name: binding.ref_name.clone(),
            run_id: binding.run_id.clone(),
            quality,
            quality_n,
            calls: agg.calls,
            input_tokens: agg.input_tokens,
            output_tokens: agg.output_tokens,
            estimated_usd: agg.estimated_usd,
            pricing_version: agg.pricing_version,
            latency: agg.latency,
            origins,
            errors: agg.requests_error + agg.requests_timeout,
        });
    }
    rows
}

fn build_project(anchor: &Path, entry: &eval_core::workspace::ProjectEntry) -> ProjectReport {
    let repo_dir = entry.repo_dir(anchor);
    if !repo_dir.is_dir() {
        return ProjectReport {
            id: entry.id.clone(),
            name: entry.name.clone(),
            repo: entry.repo.clone(),
            available: false,
            unavailability: Some(format!("repository not found: {}", repo_dir.display())),
            suites: Vec::new(),
            cases_total: 0,
            cases_passed: 0,
            cases_failed: 0,
            cases_infra: 0,
            last_success_ms: None,
            last_failure_ms: None,
            history: Vec::new(),
            usage: None,
        };
    }
    let store = RunStore::new(entry.store_dir(anchor));
    // Explicit suites win; otherwise discover `**/suite.yaml` under the
    // repo so registering a project needs only id + name + repo.
    let suite_paths = if entry.suites.is_empty() {
        discover_suites(&repo_dir, 3)
    } else {
        entry.suites.clone()
    };
    let mut suites = Vec::new();
    for suite_path in &suite_paths {
        suites.push(build_suite(anchor, entry, &store, suite_path));
    }
    let mut report = ProjectReport {
        id: entry.id.clone(),
        name: entry.name.clone(),
        repo: entry.repo.clone(),
        available: true,
        unavailability: None,
        suites,
        cases_total: 0,
        cases_passed: 0,
        cases_failed: 0,
        cases_infra: 0,
        last_success_ms: None,
        last_failure_ms: None,
        history: Vec::new(),
        usage: None,
    };
    for suite in &report.suites {
        // Case counts track the newest run per suite (history never
        // double-counts across refs).
        if let Some(newest) = newest_ref(&suite.refs) {
            report.cases_total += newest.cases_total;
            report.cases_passed += newest.cases_passed;
            report.cases_failed += newest.cases_failed;
            report.cases_infra += newest.cases_infra;
            if newest.pass {
                report.last_success_ms = Some(
                    report
                        .last_success_ms
                        .map_or(newest.started_at_ms, |m: u64| m.max(newest.started_at_ms)),
                );
            } else {
                report.last_failure_ms = Some(
                    report
                        .last_failure_ms
                        .map_or(newest.started_at_ms, |m: u64| m.max(newest.started_at_ms)),
                );
            }
        }
        report.history.extend(suite.history.clone());
    }
    // Last success/failure scan full history (newest-only counts would
    // hide a past success behind a failing head).
    for h in &report.history {
        if h.pass {
            report.last_success_ms = Some(
                report
                    .last_success_ms
                    .map_or(h.started_at_ms, |m: u64| m.max(h.started_at_ms)),
            );
        } else {
            report.last_failure_ms = Some(
                report
                    .last_failure_ms
                    .map_or(h.started_at_ms, |m: u64| m.max(h.started_at_ms)),
            );
        }
    }
    report
        .history
        .sort_by_key(|h| std::cmp::Reverse(h.started_at_ms));
    report.history.truncate(HISTORY_PER_PROJECT);
    report
}

fn build_suite(
    anchor: &Path,
    entry: &eval_core::workspace::ProjectEntry,
    store: &RunStore,
    suite_rel: &str,
) -> ProjectSuiteReport {
    let suite_path = entry.suite_path(anchor, suite_rel);
    let stem = suite_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| suite_rel.to_string());
    let suite = Suite::load(&suite_path);
    let (suite_id, policy, available, note) = match &suite {
        Ok(suite) => (suite.suite.clone(), suite.gate.clone(), true, None),
        Err(e) => (
            stem,
            GatePolicy::default(),
            false,
            Some(format!("suite unreadable: {e}")),
        ),
    };
    let mut report = ProjectSuiteReport {
        suite_id: suite_id.clone(),
        suite_path: suite_rel.to_string(),
        available,
        note,
        refs: Vec::new(),
        history: Vec::new(),
        trend: Vec::new(),
        regression: None,
        regression_note: None,
    };
    // Refs under runs/<suite_id>/.
    let runs_dir = store.root().join("runs").join(&suite_id);
    let mut ref_names: Vec<String> = match std::fs::read_dir(&runs_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    ref_names.sort();
    for ref_name in &ref_names {
        let ref_dir = runs_dir.join(ref_name);
        let mut run_ids: Vec<String> = match std::fs::read_dir(&ref_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("run-"))
                .map(|n| n.trim_start_matches("run-").to_string())
                .collect(),
            Err(_) => Vec::new(),
        };
        run_ids.sort();
        for (i, run_id) in run_ids.iter().rev().enumerate() {
            let run_dir = ref_dir.join(format!("run-{run_id}"));
            if i == 0 {
                report
                    .refs
                    .push(load_ref(store, &suite_id, ref_name, run_id, &run_dir));
            }
            let need_history = report.history.len() < HISTORY_PER_REF * ref_names.len();
            let need_trend = report.trend.len() < TREND_LIMIT;
            if need_history || need_trend {
                match store.load(&suite_id, ref_name, run_id) {
                    Ok(record) => {
                        if need_history {
                            report.history.push(HistoryEntry {
                                suite: suite_id.clone(),
                                ref_name: ref_name.clone(),
                                run_id: run_id.clone(),
                                started_at_ms: record.started_at_ms,
                                pass: record.pass(),
                                pass_rate: record.pass_rate(),
                                code_version: record.code_version.clone(),
                            });
                        }
                        if need_trend {
                            let lane = store
                                .load_manifest(&suite_id, ref_name, run_id)
                                .ok()
                                .flatten()
                                .and_then(|m| m.lane)
                                .map(|lane| lane.as_str().to_string());
                            report
                                .trend
                                .push(trend_point(&suite_id, ref_name, run_id, &record, lane));
                        }
                    }
                    Err(_) => {
                        if need_history {
                            report.history.push(HistoryEntry {
                                suite: suite_id.clone(),
                                ref_name: ref_name.clone(),
                                run_id: run_id.clone(),
                                started_at_ms: 0,
                                pass: false,
                                pass_rate: 0.0,
                                code_version: "unreadable".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    report
        .history
        .sort_by_key(|h| std::cmp::Reverse(h.started_at_ms));
    report
        .trend
        .sort_by_key(|t| std::cmp::Reverse(t.started_at_ms));
    report.trend.truncate(TREND_LIMIT);
    let (regression, note) = regression_state(store, &suite_id, &policy, &report.refs);
    report.regression = regression;
    report.regression_note = Some(note);
    report
}

fn regression_state(
    store: &RunStore,
    suite_id: &str,
    policy: &GatePolicy,
    refs: &[SuiteRefReport],
) -> (Option<bool>, String) {
    let pointer = match store.baseline(suite_id) {
        Ok(pointer) => pointer,
        Err(_) => return (None, "no baseline set".to_string()),
    };
    let Some(candidate_ref) = newest_ref(refs) else {
        return (None, "no candidate runs".to_string());
    };
    if pointer.run_id == candidate_ref.run_id && pointer.ref_name == candidate_ref.ref_name {
        return (Some(false), "candidate is the baseline".to_string());
    }
    if let Some(error) = &candidate_ref.manifest_error {
        return (
            None,
            format!("not compared: candidate manifest unreadable: {error}"),
        );
    }
    let baseline = match store.load(suite_id, &pointer.ref_name, &pointer.run_id) {
        Ok(baseline) => baseline,
        Err(e) => {
            return (
                None,
                format!("baseline pointer dangling ({e}); re-run baseline"),
            );
        }
    };
    let candidate = match store.load(suite_id, &candidate_ref.ref_name, &candidate_ref.run_id) {
        Ok(candidate) => candidate,
        Err(e) => return (None, format!("candidate unloadable: {e}")),
    };
    let baseline_manifest = match store.load_manifest(suite_id, &pointer.ref_name, &pointer.run_id)
    {
        Ok(manifest) => manifest,
        Err(e) => {
            return (
                None,
                format!("not compared: baseline manifest unreadable: {e}"),
            );
        }
    };
    if let Some(reason) = comparison_blocker(
        &baseline,
        &candidate,
        baseline_manifest.as_ref(),
        candidate_ref.manifest.as_ref(),
    ) {
        return (None, format!("not compared: {reason}"));
    }
    let decision = crate::compare::decide_gate(Some(&baseline), &candidate, policy);
    (Some(!decision.pass), decision.what_changed)
}

/// Directories never worth descending into during discovery.
const DISCOVERY_SKIP: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "eval-runs",
    "output",
    "dashboard",
    "dist",
    "build",
];

/// Find `suite.yaml` files under `root` (bounded depth), returned as paths
/// relative to `root` with forward slashes. Deterministic (sorted).
pub fn discover_suites(root: &Path, max_depth: usize) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, depth: usize, max_depth: usize, out: &mut Vec<String>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        let mut subdirs = Vec::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_file() && name == "suite.yaml" {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            } else if path.is_dir()
                && depth < max_depth
                && !DISCOVERY_SKIP.contains(&name.as_str())
                && !name.starts_with('.')
            {
                subdirs.push(path);
            }
        }
        subdirs.sort();
        for sub in subdirs {
            walk(&sub, root, depth + 1, max_depth, out);
        }
    }
    let mut out = Vec::new();
    walk(root, root, 0, max_depth, &mut out);
    out.sort();
    out
}

fn trend_point(
    suite_id: &str,
    ref_name: &str,
    run_id: &str,
    record: &RunRecord,
    lane: Option<String>,
) -> TrendPoint {
    let metrics = RunMetrics::from_record(record);
    let latency = metrics.latency.as_ref().map(|l| LatencyStats {
        n: l.n as usize,
        min_ms: l.min_ms,
        p50_ms: l.p50_ms,
        p95_ms: l.p95_ms,
        max_ms: l.max_ms,
    });
    TrendPoint {
        suite: suite_id.to_string(),
        ref_name: ref_name.to_string(),
        run_id: run_id.to_string(),
        started_at_ms: record.started_at_ms,
        pass: record.pass(),
        pass_rate: record.pass_rate(),
        cases_total: record.cases.len(),
        latency,
        tokens: TokenTotals {
            tokens_in: metrics.usage.tokens_in,
            tokens_out: metrics.usage.tokens_out,
            cost_usd: metrics.usage.cost_usd,
            reporting_trials: metrics.usage.reporting_trials.unwrap_or(0) as usize,
            total_trials: metrics.usage.total_trials.unwrap_or(0) as usize,
        },
        lane,
    }
}

pub fn comparison_blocker(
    baseline: &RunRecord,
    candidate: &RunRecord,
    baseline_manifest: Option<&RunManifest>,
    candidate_manifest: Option<&RunManifest>,
) -> Option<String> {
    if baseline.suite_version != candidate.suite_version {
        return Some(format!(
            "suite version {} -> {}",
            baseline.suite_version, candidate.suite_version
        ));
    }
    let (Some(base), Some(cand)) = (baseline_manifest, candidate_manifest) else {
        return None;
    };
    if let (Some(base_lane), Some(cand_lane)) = (base.lane, cand.lane) {
        if base_lane != cand_lane {
            return Some(format!(
                "lane {} -> {}",
                base_lane.as_str(),
                cand_lane.as_str()
            ));
        }
    }
    if let (Some(base_free), Some(cand_free)) = (base.free_only, cand.free_only) {
        if base_free != cand_free {
            return Some(format!("free_only {base_free} -> {cand_free}"));
        }
    }
    if let (Some(base_model), Some(cand_model)) = (base.model.as_ref(), cand.model.as_ref()) {
        if base_model != cand_model {
            return Some(format!(
                "model {}:{} -> {}:{}",
                base_model.provider.as_deref().unwrap_or("n/a"),
                base_model.model,
                cand_model.provider.as_deref().unwrap_or("n/a"),
                cand_model.model
            ));
        }
    }
    if let (Some(base_eval), Some(cand_eval)) = (&base.eval_version, &cand.eval_version) {
        if base_eval != cand_eval {
            return Some(format!("eval version {base_eval} -> {cand_eval}"));
        }
    }
    None
}

fn newest_ref(refs: &[SuiteRefReport]) -> Option<&SuiteRefReport> {
    refs.iter()
        .filter(|r| r.load_error.is_none())
        .max_by_key(|r| r.started_at_ms)
}

fn load_ref(
    store: &RunStore,
    suite_id: &str,
    ref_name: &str,
    run_id: &str,
    run_dir: &std::path::Path,
) -> SuiteRefReport {
    let empty_tokens = TokenTotals {
        total_trials: 0,
        ..Default::default()
    };
    let record = match store.load(suite_id, ref_name, run_id) {
        Ok(record) => record,
        Err(e) => {
            return SuiteRefReport {
                ref_name: ref_name.to_string(),
                run_id: run_id.to_string(),
                started_at_ms: 0,
                code_version: "unreadable".to_string(),
                pass: false,
                pass_rate: 0.0,
                cases_total: 0,
                cases_passed: 0,
                cases_failed: 0,
                cases_infra: 0,
                trials_total: 0,
                latency: None,
                tokens: empty_tokens,
                failed_cases: Vec::new(),
                failure_classes: Vec::new(),
                quality: None,
                stability: None,
                manifest: None,
                manifest_error: None,
                load_error: Some(e.to_string()),
                run_dir: run_dir.display().to_string(),
            };
        }
    };
    let mut report = summarize_record(record, ref_name, run_id, run_dir);
    match store.load_manifest(suite_id, ref_name, run_id) {
        Ok(manifest) => report.manifest = manifest,
        Err(e) => report.manifest_error = Some(e.to_string()),
    }
    report
}

fn summarize_record(
    record: RunRecord,
    ref_name: &str,
    run_id: &str,
    run_dir: &std::path::Path,
) -> SuiteRefReport {
    let metrics = RunMetrics::from_record(&record);
    let mut failed_cases = Vec::new();
    for case in &record.cases {
        if case.pass {
            continue;
        }
        let statuses = case
            .trials
            .iter()
            .map(|t| t.status.as_str().to_string())
            .collect();
        let reasons: Vec<String> = case
            .grades
            .iter()
            .filter(|g| !g.pass)
            .map(|g| format!("{}: {}", g.grader, g.evidence))
            .chain(case.trials.iter().filter_map(|t| {
                t.error_message.as_ref().map(|m| {
                    format!(
                        "trial {} {} [{}]: {m}",
                        t.trial,
                        t.status.as_str(),
                        t.error_code.as_deref().unwrap_or("error")
                    )
                })
            }))
            .collect();
        failed_cases.push(FailedCase {
            case_id: case.case_id.clone(),
            statuses,
            reasons,
        });
    }
    let trials_total = metrics
        .trials
        .completed
        .map(|n| n as usize)
        .unwrap_or_else(|| record.cases.iter().map(|c| c.trials.len()).sum());
    let latency = metrics.latency.as_ref().map(|l| LatencyStats {
        n: l.n as usize,
        min_ms: l.min_ms,
        p50_ms: l.p50_ms,
        p95_ms: l.p95_ms,
        max_ms: l.max_ms,
    });
    let usage = &metrics.usage;
    SuiteRefReport {
        ref_name: ref_name.to_string(),
        run_id: run_id.to_string(),
        started_at_ms: record.started_at_ms,
        code_version: record.code_version.clone(),
        pass: record.pass(),
        pass_rate: record.pass_rate(),
        cases_total: metrics
            .outcomes
            .total
            .map(|n| n as usize)
            .unwrap_or(record.cases.len()),
        cases_passed: metrics
            .outcomes
            .passed
            .map(|n| n as usize)
            .unwrap_or_else(|| record.cases.iter().filter(|c| c.pass).count()),
        cases_failed: metrics.outcomes.failed.unwrap_or(0) as usize,
        cases_infra: metrics.outcomes.infra.unwrap_or(0) as usize,
        trials_total,
        latency,
        tokens: TokenTotals {
            tokens_in: usage.tokens_in,
            tokens_out: usage.tokens_out,
            cost_usd: usage.cost_usd,
            reporting_trials: usage.reporting_trials.unwrap_or(0) as usize,
            total_trials: usage.total_trials.unwrap_or(0) as usize,
        },
        failed_cases,
        failure_classes: failure_counts(&metrics),
        quality: quality_view(&metrics),
        stability: stability_view(&metrics),
        manifest: None,
        manifest_error: None,
        load_error: None,
        run_dir: run_dir.display().to_string(),
    }
}
