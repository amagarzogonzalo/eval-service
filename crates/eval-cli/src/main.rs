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
