//! `eval`: run suites, compare runs against baselines, render reports,
//! scaffold new adapters.
//!
//! Exit codes: 0 pass, 1 regression/failure, 2 infra/protocol error.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use eval_core::{
    BaselinePointer, GateDecision, RunRecord, RunStore, Suite, TargetSpec, TrialObservation,
    record::assemble_case,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "eval",
    version = env!("EVAL_BUILD_INFO"),
    about = "Reusable Rust evaluation platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a suite, grade it, and store the record.
    Run {
        /// Suite YAML file.
        suite: PathBuf,
        /// Run ref (baseline pointer namespace). Default `work`.
        #[arg(long, default_value = "work")]
        r#ref: String,
        /// Store root directory. Default `eval-runs`.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Override the target argv (e.g. point at a local adapter).
        #[arg(long)]
        target: Vec<String>,
        /// Point the suite baseline at this run after storing.
        #[arg(long)]
        set_baseline: bool,
        /// Worker slots (default: min(4, parallelism)). Use 1 to force
        /// shared-process sequencing.
        #[arg(long)]
        workers: Option<usize>,
        /// Machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Compare two runs and decide the gate.
    Compare {
        /// Suite YAML file (gate policy + case names).
        #[arg(long)]
        suite: PathBuf,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Baseline ref. Default `baseline`.
        #[arg(long, default_value = "baseline")]
        baseline_ref: String,
        /// Candidate ref. Default `work`.
        #[arg(long, default_value = "work")]
        candidate_ref: String,
        /// Explicit run ids (default: latest of each ref).
        #[arg(long)]
        baseline_id: Option<String>,
        #[arg(long)]
        candidate_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Render stored runs (one `--suite` per suite section).
    Report {
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Suite name(s) to render.
        #[arg(long, required = true)]
        suite: Vec<String>,
        /// Run ref. Default `work`.
        #[arg(long, default_value = "work")]
        r#ref: String,
        /// Explicit run id (default: latest).
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Scaffold a new adapter (script + suite + readme) in `--dir`.
    Init {
        /// Adapter name.
        name: String,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// List registered projects and their evaluation status.
    Projects {
        /// Workspace file (default `eval-workspace.toml`).
        #[arg(long, default_value = "eval-workspace.toml")]
        workspace: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Render a read-only static dashboard from canonical run stores.
    Dashboard {
        /// Workspace file (default `eval-workspace.toml`).
        #[arg(long, default_value = "eval-workspace.toml")]
        workspace: PathBuf,
        /// Output directory for `index.html`.
        #[arg(long, default_value = "dashboard")]
        out: PathBuf,
    },
    /// Record LLM-call usage events (JSONL import).
    Usage {
        /// JSONL event file (one `llm/v1` event per line).
        file: PathBuf,
        /// Project id the batch belongs to.
        #[arg(long)]
        project: String,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Tag stamped on events that lack one (benchmark/variant label).
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Report aggregated LLM usage for a project.
    UsageReport {
        /// Project id.
        #[arg(long)]
        project: String,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Pricing table (default `<store>/pricing.toml`, else unknown costs).
        #[arg(long)]
        pricing: Option<PathBuf>,
        /// Filter to one model.
        #[arg(long)]
        model: Option<String>,
        /// Filter to one tag.
        #[arg(long)]
        tag: Option<String>,
        /// Filter to one traffic context: production, benchmark, or dev.
        /// Default aggregates all contexts (labeled as such).
        #[arg(long)]
        context: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run a loopback-only local collector for usage events.
    Collect {
        /// Port to bind on 127.0.0.1.
        #[arg(long, default_value_t = 4317)]
        port: u16,
        /// Store root directory.
        #[arg(long, default_value = "eval-runs")]
        store: PathBuf,
        /// Optional shared-token file (Bearer auth when present).
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("eval: error: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Run {
            suite,
            r#ref,
            store,
            target,
            set_baseline,
            workers,
            json,
        } => cmd_run(&suite, &r#ref, &store, target, set_baseline, workers, json).await,
        Command::Compare {
            suite,
            store,
            baseline_ref,
            candidate_ref,
            baseline_id,
            candidate_id,
            json,
        } => cmd_compare(
            &suite,
            &store,
            &baseline_ref,
            &candidate_ref,
            baseline_id,
            candidate_id,
            json,
        ),
        Command::Report {
            store,
            suite,
            r#ref,
            run_id,
            json,
        } => cmd_report(&store, &suite, &r#ref, run_id, json),
        Command::Init { name, dir } => cmd_init(&name, &dir),
        Command::Projects { workspace, json } => cmd_projects(&workspace, json),
        Command::Dashboard { workspace, out } => cmd_dashboard(&workspace, &out),
        Command::Usage { file, project, store, tag, json } => {
            cmd_usage_import(&file, &project, &store, tag.as_deref(), json)
        }
        Command::UsageReport { project, store, pricing, model, tag, context, json } => {
            cmd_usage_report(&project, &store, pricing.as_deref(), model.as_deref(), tag.as_deref(), context.as_deref(), json)
        }
        Command::Collect { port, store, token_file } => {
            cmd_collect(port, &store, token_file.as_deref()).await
        }
    }
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    suite_path: &Path,
    ref_name: &str,
    store_root: &Path,
    target_override: Vec<String>,
    set_baseline: bool,
    workers: Option<usize>,
    as_json: bool,
) -> Result<i32> {
    let suite = Suite::load(suite_path).context("loading suite")?;
    let mut target: TargetSpec = suite.target.clone().context("suite has no target")?;
    if !target_override.is_empty() {
        target.argv = target_override;
    }
    let mut policy = eval_runner::RunPolicy::default_for(suite.env_allowlist.clone());
    if let Some(w) = workers {
        policy.workers = w.max(1);
    }
    let suite_dir = suite_path.parent().unwrap_or(Path::new("."));
    let files_root = std::env::current_dir().context("resolving cwd")?;

    let raw = eval_runner::run_suite(&suite, &target, &policy, suite_dir)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (mut record, observations) = assemble(&suite, target, &files_root, ref_name, raw);

    let store = RunStore::new(store_root);
    let run_id = store
        .save(&mut record, &observations)
        .context("storing run")?;
    if set_baseline {
        store
            .set_baseline(&BaselinePointer {
                suite: suite.suite.clone(),
                ref_name: ref_name.to_string(),
                run_id: run_id.clone(),
            })
            .context("setting baseline")?;
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        print_human(&record, None);
        println!("run: {run_id} (ref {ref_name})");
    }
    Ok(if record.pass() { 0 } else { 1 })
}

fn assemble(
    suite: &Suite,
    target: TargetSpec,
    files_root: &Path,
    ref_name: &str,
    raw: eval_runner::SuiteRun,
) -> (RunRecord, Vec<TrialObservation>) {
    let mut cases = Vec::new();
    let mut observations = Vec::new();
    for spec in &suite.cases {
        let trials = raw
            .cases
            .iter()
            .find(|c| c.case_id == spec.id)
            .map(|c| c.trials.clone())
            .unwrap_or_default();
        let grades = eval_graders::grade_case(spec, &trials, files_root);
        observations.extend(trials.clone());
        cases.push(assemble_case(spec, trials, grades));
    }
    let trials_total = cases.iter().map(|c| c.trials.len()).sum();
    (
        RunRecord {
            schema_version: eval_core::record::SCHEMA_VERSION,
            run_id: String::new(),
            suite: suite.suite.clone(),
            suite_version: suite.version,
            code_version: RunRecord::code_version(),
            ref_name: ref_name.to_string(),
            started_at_ms: raw.started_ms,
            target: Some(target),
            files_root: files_root.display().to_string(),
            trials_total,
            cases,
            gate: None,
        },
        observations,
    )
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

fn cmd_compare(
    suite_path: &Path,
    store_root: &Path,
    baseline_ref: &str,
    candidate_ref: &str,
    baseline_id: Option<String>,
    candidate_id: Option<String>,
    as_json: bool,
) -> Result<i32> {
    let suite = Suite::load(suite_path).context("loading suite")?;
    let store = RunStore::new(store_root);
    let resolve = |ref_name: &str, id: Option<String>| -> Result<(String, RunRecord)> {
        let id = match id {
            Some(id) => id,
            None => store
                .latest(&suite.suite, ref_name)?
                .context(format!("no runs for {}/{ref_name}", suite.suite))?,
        };
        Ok((id.clone(), store.load(&suite.suite, ref_name, &id)?))
    };
    let (base_id, baseline) = resolve(baseline_ref, baseline_id)?;
    let (cand_id, mut candidate) = resolve(candidate_ref, candidate_id)?;

    let decision: GateDecision =
        eval_graders::decide_gate(Some(&baseline), &candidate, &suite.gate);
    let pass = decision.pass;
    candidate.gate = Some(decision.clone());
    // Persist the decision alongside the candidate run.
    let decision_path = store_root
        .join("runs")
        .join(&suite.suite)
        .join(candidate_ref)
        .join(format!("run-{cand_id}"))
        .join("decision.json");
    std::fs::write(&decision_path, serde_json::to_string_pretty(&decision)?)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&decision)?);
    } else {
        print_decision(&suite.suite, &base_id, &cand_id, &decision);
    }
    Ok(if pass { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------

fn cmd_report(
    store_root: &Path,
    suites: &[String],
    ref_name: &str,
    run_id: Option<String>,
    as_json: bool,
) -> Result<i32> {
    let store = RunStore::new(store_root);
    let mut records = Vec::new();
    for suite in suites {
        let id = match &run_id {
            Some(id) => id.clone(),
            None => store
                .latest(suite, ref_name)?
                .context(format!("no runs for {suite}/{ref_name}"))?,
        };
        records.push(store.load(suite, ref_name, &id)?);
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        for record in &records {
            print_human(record, None);
            println!();
        }
    }
    Ok(if records.iter().all(|r| r.pass()) {
        0
    } else {
        1
    })
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn cmd_init(name: &str, dir: &Path) -> Result<i32> {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("adapter"))?;
    std::fs::write(root.join("adapter").join("target.py"), ADAPTER_TEMPLATE)?;
    std::fs::write(root.join("suite.yaml"), suite_template(name))?;
    std::fs::write(root.join("README.md"), readme_template(name))?;
    println!("scaffolded {} in {}", name, root.display());
    println!(
        "next: cd {} and edit adapter/target.py, then `eval run suite.yaml`",
        root.display()
    );
    println!("(target argv and file paths resolve from the directory you run eval in)");
    Ok(0)
}

// ---------------------------------------------------------------------------
// projects + dashboard
// ---------------------------------------------------------------------------

fn load_portfolio(workspace: &Path) -> Result<eval_graders::portfolio::PortfolioReport> {
    let file = eval_core::WorkspaceFile::load(workspace).context("loading workspace")?;
    Ok(eval_graders::portfolio::build_portfolio(workspace, &file))
}

fn cmd_projects(workspace: &Path, as_json: bool) -> Result<i32> {
    let report = load_portfolio(workspace)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(0);
    }
    println!(
        "workspace {} ({} projects)",
        report.workspace_name,
        report.projects.len()
    );
    for project in &report.projects {
        if !project.available {
            println!(
                "  [unavailable] {} ({}) — {}",
                project.name,
                project.id,
                project
                    .unavailability
                    .as_deref()
                    .unwrap_or("unknown reason")
            );
            continue;
        }
        let regressed = project
            .suites
            .iter()
            .filter(|s| s.regression == Some(true))
            .count();
        let mark = if project.cases_infra > 0 {
            "error"
        } else if project.cases_failed > 0 || regressed > 0 {
            "FAIL"
        } else if project.cases_total == 0 {
            "no runs"
        } else {
            "ok"
        };
        println!(
            "  [{mark}] {} ({}) — {} suites, {} cases ({} pass / {} fail / {} infra), regressed suites: {regressed}",
            project.name,
            project.id,
            project.suites.len(),
            project.cases_total,
            project.cases_passed,
            project.cases_failed,
            project.cases_infra,
        );
        for suite in &project.suites {
            let latest = suite
                .history
                .first()
                .map(|h| {
                    format!(
                        "{}:{} {}",
                        h.suite,
                        h.ref_name,
                        &h.run_id[..h.run_id.len().min(13)]
                    )
                })
                .unwrap_or_else(|| "no runs".to_string());
            println!("      suite {} — latest {latest}", suite.suite_id);
        }
    }
    let t = &report.totals;
    println!(
        "totals: {}/{} projects available, {} cases pass / {} fail / {} infra",
        t.projects_available, t.projects_total, t.cases_passed, t.cases_failed, t.cases_infra
    );
    Ok(0)
}

fn cmd_dashboard(workspace: &Path, out: &Path) -> Result<i32> {
    let report = load_portfolio(workspace)?;
    let html = eval_graders::dashboard::render_portfolio(&report);
    std::fs::create_dir_all(out).context("creating output dir")?;
    let page = out.join("index.html");
    std::fs::write(&page, html).context("writing dashboard")?;
    println!(
        "dashboard: {} projects, {} suites",
        report.projects.len(),
        report.totals.suites_total
    );
    println!("wrote {}", page.display());
    println!(
        "open it with: start {}  (Windows) / open {} (macOS) / xdg-open {} (Linux)",
        page.display(),
        page.display(),
        page.display()
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// usage import / report / collect
// ---------------------------------------------------------------------------

fn cmd_usage_import(
    file: &Path,
    project: &str,
    store_root: &Path,
    tag: Option<&str>,
    as_json: bool,
) -> Result<i32> {
    let store = eval_usage::UsageStore::new(store_root);
    let summary = store
        .import_file(project, file, tag)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "usage import: {} accepted, {} duplicates, {} rejected (project {project})",
            summary.accepted,
            summary.duplicates,
            summary.rejected.len()
        );
        for rejection in &summary.rejected {
            println!("  rejected line {}: {}", rejection.line, rejection.reason);
        }
    }
    Ok(0)
}

fn resolve_pricing(store_root: &Path, explicit: Option<&Path>) -> eval_usage::PriceTable {
    if let Some(path) = explicit {
        return eval_usage::PriceTable::load(path).unwrap_or_else(|e| {
            eprintln!("eval: warning: pricing file unreadable ({e}); costs stay unknown");
            eval_usage::PriceTable::empty()
        });
    }
    let candidate = store_root.join("pricing.toml");
    if candidate.exists() {
        return eval_usage::PriceTable::load(&candidate).unwrap_or_else(|e| {
            eprintln!("eval: warning: pricing file unreadable ({e}); costs stay unknown");
            eval_usage::PriceTable::empty()
        });
    }
    eval_usage::PriceTable::empty()
}

#[allow(clippy::too_many_arguments)]
fn cmd_usage_report(
    project: &str,
    store_root: &Path,
    pricing_path: Option<&Path>,
    model_filter: Option<&str>,
    tag_filter: Option<&str>,
    context_filter: Option<&str>,
    as_json: bool,
) -> Result<i32> {
    let context = match context_filter {
        None => None,
        Some("production") => Some(eval_usage::UsageContext::Production),
        Some("benchmark") => Some(eval_usage::UsageContext::Benchmark),
        Some("dev") => Some(eval_usage::UsageContext::Dev),
        Some(other) => anyhow::bail!("unknown context {other:?} (production|benchmark|dev)"),
    };
    let store = eval_usage::UsageStore::new(store_root);
    let (mut events, skipped) = store
        .read_events(project)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(model) = model_filter {
        events.retain(|e| e.model == model);
    }
    if let Some(tag) = tag_filter {
        events.retain(|e| e.tag.as_deref() == Some(tag));
    }
    let pricing = resolve_pricing(store_root, pricing_path);
    let usage = eval_usage::aggregate_project(project, &events, &pricing, context);
    let (tasks, tasks_skipped) = eval_usage::aggregate_tasks(&events, &pricing, context);
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "usage": usage,
                "tasks": tasks,
                "tasks_without_id_skipped": tasks_skipped,
                "unreadable_stored_lines_skipped": skipped,
            }))?
        );
    } else {
        println!(
            "context filter: {}",
            context_filter.unwrap_or("all (production + benchmark + dev)")
        );
        print_usage_human(&usage, &tasks, skipped);
    }
    Ok(0)
}

fn print_usage_human(
    usage: &eval_usage::ProjectUsage,
    tasks: &[eval_usage::TaskRollup],
    skipped: usize,
) {
    println!("usage {} — {} events, {} calls, {} requests", usage.project, usage.events, usage.calls, usage.requests);
    println!(
        "  requests: {} ok / {} error / {} timeout, {} retries",
        usage.requests_ok, usage.requests_error, usage.requests_timeout, usage.retries
    );
    println!(
        "  tokens: in {} / out {}",
        opt_sum(&usage.input_tokens),
        opt_sum(&usage.output_tokens)
    );
    println!(
        "  cost: reported {} / estimated {} (pricing v{})",
        opt_usd(&usage.reported_usd),
        opt_usd(&usage.estimated_usd),
        usage.pricing_version.unwrap_or(0)
    );
    println!("  latency: {}", opt_latency(usage.latency.as_ref()));
    for row in &usage.by_model {
        println!(
            "  model {}:{} [{}] — {} calls, {} requests, success {}",
            row.provider,
            row.model,
            row.origin,
            row.calls,
            row.requests,
            row.success_rate.map_or("n/a".to_string(), |r| format!("{:.0}%", r * 100.0))
        );
        println!(
            "    tokens in {} / out {}; cost reported {} / estimated {}; latency {}; errors {} timeouts {} retries {}",
            opt_sum(&row.input_tokens),
            opt_sum(&row.output_tokens),
            opt_usd(&row.reported_usd),
            opt_usd(&row.estimated_usd),
            opt_latency(row.latency.as_ref()),
            row.errors,
            row.timeouts,
            row.retries
        );
    }
    if !tasks.is_empty() {
        println!("  tasks:");
        for task in tasks {
            println!(
                "    {} — {} calls, {} requests, {} ({} models), tokens in {} / out {}, cost est {}, duration {}",
                task.task_id,
                task.calls,
                task.requests,
                if task.success { "success" } else { "FAILED" },
                task.models.join(","),
                opt_sum(&task.input_tokens),
                opt_sum(&task.output_tokens),
                opt_usd(&task.estimated_usd),
                task.duration_ms.map_or("n/a".to_string(), |d| format!("{d}ms"))
            );
        }
    }
    if skipped > 0 {
        println!("  note: {skipped} unreadable stored lines skipped");
    }
}

fn opt_sum(sum: &eval_usage::aggregate::OptSum) -> String {
    match sum.value {
        Some(v) => format!("{v:.0} ({} reporting / {} total)", sum.reporting, sum.total),
        None => format!("n/a (0/{} reporting)", sum.total),
    }
}

fn opt_usd(sum: &eval_usage::aggregate::OptSum) -> String {
    match sum.value {
        Some(v) => format!("${v:.4} ({} reporting)", sum.reporting),
        None => "n/a".to_string(),
    }
}

fn opt_latency(latency: Option<&eval_usage::LatencyDist>) -> String {
    match latency {
        Some(l) => format!("p50 {}ms / p95 {}ms (n={})", l.p50_ms, l.p95_ms, l.n),
        None => "n/a".to_string(),
    }
}

async fn cmd_collect(port: u16, store_root: &Path, token_file: Option<&Path>) -> Result<i32> {
    let config = eval_usage::collector::CollectorConfig {
        port,
        store_root: store_root.to_path_buf(),
        token_file: token_file.map(Path::to_path_buf),
    };
    println!("collecting usage events on 127.0.0.1:{port} (POST /v1/events; Ctrl-C to stop)");
    eval_usage::collector::serve(config)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

fn print_human(record: &RunRecord, decision: Option<&GateDecision>) {
    println!(
        "suite {} v{}  ref {}",
        record.suite, record.suite_version, record.ref_name
    );
    println!(
        "run {}  {} cases / {} trials  pass rate {:.2}",
        if record.run_id.is_empty() {
            "(unstored)"
        } else {
            &record.run_id
        },
        record.cases.len(),
        record.trials_total,
        record.pass_rate()
    );
    for case in &record.cases {
        let mark = if case.pass { "ok  " } else { "FAIL" };
        let statuses: Vec<String> = case
            .trials
            .iter()
            .map(|t| t.status.as_str().to_string())
            .collect();
        let walls: Vec<String> = case
            .trials
            .iter()
            .map(|t| format!("{}ms", t.wall_ms))
            .collect();
        println!(
            "  [{mark}] {}  [{}]  wall [{}]",
            case.case_id,
            statuses.join(","),
            walls.join(",")
        );
        for grade in &case.grades {
            let gmark = if grade.pass { "pass" } else { "FAIL" };
            println!("         {gmark:4} {} — {}", grade.grader, grade.evidence);
        }
    }
    if let Some(gate) = decision.or(record.gate.as_ref()) {
        print_decision(&record.suite, "baseline", &record.run_id, gate);
    }
}

fn print_decision(suite: &str, base_id: &str, cand_id: &str, decision: &GateDecision) {
    let verdict = if decision.pass { "PASS" } else { "FAIL" };
    println!("gate [{verdict}] {suite}: baseline {base_id} vs candidate {cand_id}");
    println!("  what changed: {}", decision.what_changed);
    for case in &decision.affected_cases {
        println!("  affected: {case}");
    }
    for line in &decision.evidence {
        println!("  evidence: {line}");
    }
    if let Some(uncertainty) = &decision.uncertainty {
        println!("  uncertainty: {uncertainty}");
    }
}

const ADAPTER_TEMPLATE: &str = r#"#!/usr/bin/env python3
"""Minimal eval/v1 target. Reads JSONL on stdin, writes JSONL on stdout.

Logs go to stderr (never parsed). Replace `handle_case` with your tool.
"""

import json
import sys


def handle_case(case):
    text = case.get("input", {}).get("stdin_text", "")
    return {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {"text": text, "files": []},
        "metrics": {"wall_ms": 0},
    }


def main():
    mode = "worker"
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if "protocol" in msg:
            mode = msg.get("mode", "worker")
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
        elif msg.get("type") == "case":
            print(json.dumps(handle_case(msg)), flush=True)
            if mode == "oneshot":
                return
        elif msg.get("type") == "shutdown":
            return


if __name__ == "__main__":
    main()
"#;

fn suite_template(name: &str) -> String {
    format!(
        r#"suite: {name}
version: 1
target:
  argv: ["$PYTHON", "adapter/target.py"]
  mode: oneshot
env_allowlist: []
gate:
  min_pass_rate: 1.0
cases:
  - id: echo-smoke
    input: {{ stdin_text: "hello" }}
    limits: {{ timeout_ms: 5000, max_output_bytes: 65536 }}
    trials: 1
    graders:
      - kind: exact
        field: output.text
        value: "hello"
  - id: echo-stable
    input: {{ stdin_text: "same" }}
    limits: {{ timeout_ms: 5000, max_output_bytes: 65536 }}
    trials: 2
    graders:
      - kind: stability
"#
    )
}

fn readme_template(name: &str) -> String {
    format!(
        "# {name} eval suite\n\n1. Implement `adapter/target.py:handle_case`.\n\
         2. `eval run suite.yaml --ref baseline --set-baseline`\n\
         3. Change code, `eval run suite.yaml --ref work`, then\n\
         `eval compare --suite suite.yaml`.\n"
    )
}
