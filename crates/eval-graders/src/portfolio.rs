//! Cross-project portfolio aggregation.
//!
//! Reads the canonical run stores through `RunStore` — never a shadow
//! database. Unavailable projects, missing suites, and corrupt records
//! are reported as first-class entries, never silently dropped.
//! Absent metrics stay `None` (rendered `n/a`), never zero-filled.

use eval_core::{GatePolicy, RunRecord, RunStore, Suite, TrialStatus, WorkspaceFile};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How many recent runs per suite ref feed history lists.
pub const HISTORY_PER_REF: usize = 5;
/// Cap on project-level merged history.
pub const HISTORY_PER_PROJECT: usize = 20;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    /// Absolute run directory (for artifact links).
    pub run_dir: String,
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
    let mut suites = Vec::new();
    for suite_path in &entry.suites {
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
            if report.history.len() < HISTORY_PER_REF * ref_names.len() {
                match store.load(&suite_id, ref_name, run_id) {
                    Ok(record) => report.history.push(HistoryEntry {
                        suite: suite_id.clone(),
                        ref_name: ref_name.clone(),
                        run_id: run_id.clone(),
                        started_at_ms: record.started_at_ms,
                        pass: record.pass(),
                        pass_rate: record.pass_rate(),
                        code_version: record.code_version.clone(),
                    }),
                    Err(_) => report.history.push(HistoryEntry {
                        suite: suite_id.clone(),
                        ref_name: ref_name.clone(),
                        run_id: run_id.clone(),
                        started_at_ms: 0,
                        pass: false,
                        pass_rate: 0.0,
                        code_version: "unreadable".to_string(),
                    }),
                }
            }
        }
    }
    report
        .history
        .sort_by_key(|h| std::cmp::Reverse(h.started_at_ms));
    // Fresh regression recomputation against the baseline pointer.
    match store.baseline(&suite_id) {
        Ok(pointer) => {
            let candidate = newest_ref(&report.refs);
            match (
                store.load(&suite_id, &pointer.ref_name, &pointer.run_id),
                candidate,
            ) {
                (Ok(baseline), Some(candidate_ref)) => {
                    match store.load(&suite_id, &candidate_ref.ref_name, &candidate_ref.run_id) {
                        Ok(candidate_record) => {
                            if pointer.run_id == candidate_ref.run_id
                                && pointer.ref_name == candidate_ref.ref_name
                            {
                                report.regression = Some(false);
                                report.regression_note =
                                    Some("candidate is the baseline".to_string());
                            } else {
                                let decision = crate::compare::decide_gate(
                                    Some(&baseline),
                                    &candidate_record,
                                    &policy,
                                );
                                report.regression = Some(!decision.pass);
                                report.regression_note = Some(decision.what_changed.clone());
                            }
                        }
                        Err(e) => {
                            report.regression_note = Some(format!("candidate unloadable: {e}"));
                        }
                    }
                }
                (Err(e), _) => {
                    report.regression_note =
                        Some(format!("baseline pointer dangling ({e}); re-run baseline"));
                }
                (_, None) => {
                    report.regression_note = Some("no candidate runs".to_string());
                }
            }
        }
        Err(_) => {
            report.regression_note = Some("no baseline set".to_string());
        }
    }
    report
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
                load_error: Some(e.to_string()),
                run_dir: run_dir.display().to_string(),
            };
        }
    };
    summarize_record(record, ref_name, run_id, run_dir)
}

fn summarize_record(
    record: RunRecord,
    ref_name: &str,
    run_id: &str,
    run_dir: &std::path::Path,
) -> SuiteRefReport {
    let mut walls: Vec<u64> = Vec::new();
    let mut tokens_in: u64 = 0;
    let mut tokens_out: u64 = 0;
    let mut cost_usd: f64 = 0.0;
    let mut saw_in = false;
    let mut saw_out = false;
    let mut saw_cost = false;
    let mut reporting = 0;
    let mut failed_cases = Vec::new();
    let mut cases_failed = 0;
    let mut cases_infra = 0;
    for case in &record.cases {
        let mut statuses = Vec::new();
        for trial in &case.trials {
            statuses.push(trial.status.as_str().to_string());
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
        let infra = !case.pass
            && case
                .trials
                .iter()
                .any(|t| matches!(t.status, TrialStatus::Crash | TrialStatus::ProtocolError));
        if !case.pass {
            cases_failed += 1;
        }
        if infra {
            cases_infra += 1;
        }
        if !case.pass {
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
    }
    let trials_total: usize = record.cases.iter().map(|c| c.trials.len()).sum();
    SuiteRefReport {
        ref_name: ref_name.to_string(),
        run_id: run_id.to_string(),
        started_at_ms: record.started_at_ms,
        code_version: record.code_version.clone(),
        pass: record.pass(),
        pass_rate: record.pass_rate(),
        cases_total: record.cases.len(),
        cases_passed: record.cases.iter().filter(|c| c.pass).count(),
        cases_failed,
        cases_infra,
        trials_total,
        latency: latency_stats(&walls),
        tokens: TokenTotals {
            tokens_in: saw_in.then_some(tokens_in),
            tokens_out: saw_out.then_some(tokens_out),
            cost_usd: saw_cost.then_some(cost_usd),
            reporting_trials: reporting,
            total_trials: trials_total,
        },
        failed_cases,
        load_error: None,
        run_dir: run_dir.display().to_string(),
    }
}

fn latency_stats(walls: &[u64]) -> Option<LatencyStats> {
    if walls.is_empty() {
        return None;
    }
    let mut sorted = walls.to_vec();
    sorted.sort_unstable();
    let rank =
        |q: f64| sorted[((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1];
    Some(LatencyStats {
        n: sorted.len(),
        min_ms: sorted[0],
        p50_ms: rank(0.5),
        p95_ms: rank(0.95),
        max_ms: sorted[sorted.len() - 1],
    })
}
