//! Dashboard rendering tests: totals match the store (independent
//! cross-check), honest empty/missing states, escaping.

use eval_core::*;
use eval_graders::dashboard::render_portfolio;
use eval_graders::portfolio::*;
use std::path::{Path, PathBuf};

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-dash-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SUITE_YAML: &str = r#"
suite: s
version: 1
cases:
  - id: a
    input: {}
    limits: { timeout_ms: 1000, max_output_bytes: 1024 }
    graders: []
"#;

fn trial_ok(case_id: &str) -> TrialObservation {
    TrialObservation {
        case_id: case_id.into(),
        trial: 0,
        status: TrialStatus::Ok,
        exit_code: Some(0),
        wall_ms: 7,
        output: Some(serde_json::json!({"output": {"text": "x"}})),
        events: Vec::new(),
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn save(dir: &Path, suite: &str, ref_name: &str, pass: bool, started: u64) -> String {
    let trial = trial_ok("a");
    let obs = vec![trial.clone()];
    let mut record = RunRecord {
        schema_version: 1,
        run_id: String::new(),
        suite: suite.into(),
        suite_version: 1,
        code_version: "v1".into(),
        ref_name: ref_name.into(),
        started_at_ms: started,
        request_id: None,
        target: None,
        files_root: ".".into(),
        trials_total: 1,
        cases: vec![CaseRecord {
            case_id: "a".into(),
            deterministic: true,
            trials: obs.clone(),
            grades: vec![Grade {
                grader: "g".into(),
                pass,
                score: None,
                evidence: "e".into(),
            }],
            pass,
            digest: digest_trials(&obs),
        }],
        gate: None,
    };
    RunStore::new(dir.join("eval-runs"))
        .save(&mut record, &obs)
        .unwrap()
}

/// Workspace with: alpha (passing run + baseline), beta (missing repo),
/// gamma (corrupt record + failing run).
fn scaffold() -> (PathBuf, PathBuf) {
    let dir = tmpdir("scaffold");
    std::fs::write(
        dir.join("eval-workspace.toml"),
        r#"
[workspace]
name = "test-portfolio"

[[project]]
id = "alpha"
name = "Alpha <b>project</b>"
repo = "alpha"
suites = ["suite.yaml"]

[[project]]
id = "beta"
name = "Beta"
repo = "does-not-exist"
suites = []

[[project]]
id = "gamma"
name = "Gamma"
repo = "gamma"
suites = ["suite.yaml"]
"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::write(dir.join("alpha/suite.yaml"), SUITE_YAML).unwrap();
    std::fs::create_dir_all(dir.join("gamma")).unwrap();
    std::fs::write(dir.join("gamma/suite.yaml"), SUITE_YAML).unwrap();

    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let base = save(&dir.join("alpha"), "s", "baseline", true, 1000);
    store
        .set_baseline(&BaselinePointer {
            suite: "s".into(),
            ref_name: "baseline".into(),
            run_id: base,
        })
        .unwrap();
    save(&dir.join("alpha"), "s", "work", true, 2000);
    let bad = save(&dir.join("gamma"), "s", "work", false, 1000);
    let meta = dir
        .join("gamma/eval-runs/runs/s/work")
        .join(format!("run-{bad}/metadata.json"));
    std::fs::write(&meta, "garbage{").unwrap();
    (dir.clone(), dir.join("eval-workspace.toml"))
}

fn portfolio_of(ws_path: &Path) -> PortfolioReport {
    let file = WorkspaceFile::load(ws_path).unwrap();
    build_portfolio(ws_path, &file)
}

#[test]
fn totals_match_the_store() {
    let (_dir, ws_path) = scaffold();
    let report = portfolio_of(&ws_path);
    let html = render_portfolio(&report);
    // Independent cross-check: the rendered totals equal the store truth.
    assert!(html.contains(">3<"), "3 projects, {html:.200}");
    assert!(html.contains(&report.totals.cases_passed.to_string()));
    assert!(html.contains(&report.totals.cases_failed.to_string()));
    assert!(html.contains("Overall rate"));
    assert!(html.contains("weights projects by case count"));
}

fn store_totals(store: &Path) -> (usize, usize, usize) {
    let mut totals = (0usize, 0usize, 0usize);
    let runs = store.join("runs");
    let Ok(suites) = std::fs::read_dir(&runs) else {
        return totals;
    };
    for suite in suites.filter_map(|e| e.ok()) {
        let mut newest: Option<(u64, serde_json::Value)> = None;
        let Ok(refs) = std::fs::read_dir(suite.path()) else {
            continue;
        };
        for ref_dir in refs.filter_map(|e| e.ok()) {
            let Ok(ids) = std::fs::read_dir(ref_dir.path()) else {
                continue;
            };
            for run in ids.filter_map(|e| e.ok()) {
                let Ok(text) = std::fs::read_to_string(run.path().join("metadata.json")) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let started = value["started_at_ms"].as_u64().unwrap_or(0);
                if newest.as_ref().is_none_or(|(ms, _)| started > *ms) {
                    newest = Some((started, value));
                }
            }
        }
        if let Some((_, value)) = newest {
            for case in value["cases"].as_array().into_iter().flatten() {
                totals.0 += 1;
                if case["pass"].as_bool() == Some(true) {
                    totals.1 += 1;
                } else {
                    totals.2 += 1;
                }
            }
        }
    }
    totals
}

#[test]
fn totals_match_independent_store_scan() {
    let (dir, ws_path) = scaffold();
    let report = portfolio_of(&ws_path);
    let alpha = store_totals(&dir.join("alpha/eval-runs"));
    let gamma = store_totals(&dir.join("gamma/eval-runs"));
    assert_eq!(report.totals.cases_total, alpha.0 + gamma.0);
    assert_eq!(report.totals.cases_passed, alpha.1 + gamma.1);
    assert_eq!(report.totals.cases_failed, alpha.2 + gamma.2);
}

#[test]
fn manifest_sidecar_renders_lane_service_and_evidence() {
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let run_id = store.latest("s", "work").unwrap().unwrap();
    let record = store.load("s", "work", &run_id).unwrap();
    let mut manifest = eval_core::run_manifest::RunManifest::from_record(
        &record,
        Some(eval_core::run_manifest::Lane::RemoteService),
        Some("eval-1".into()),
        Some("target-9".into()),
        Some(eval_core::run_manifest::ModelRef {
            provider: Some("fixture".into()),
            model: "model-x".into(),
        }),
        Some(true),
    );
    manifest.metrics.service = Some(eval_core::run_manifest::ServiceMetrics {
        availability: Some(eval_core::run_manifest::Availability {
            attempts: Some(100),
            successes: Some(98),
            ratio: None,
        }),
        cold_start: Some(eval_core::run_manifest::LatencyStats {
            n: 2,
            min_ms: 90,
            p50_ms: 120,
            p95_ms: 180,
            max_ms: 180,
        }),
        queue_wait: Some(eval_core::run_manifest::LatencyStats {
            n: 2,
            min_ms: 5,
            p50_ms: 10,
            p95_ms: 20,
            max_ms: 20,
        }),
    });
    manifest.evidence.trace_id = Some("trace-1".into());
    manifest.evidence.trace_url = Some("javascript:alert(1)".into());
    manifest.evidence.task_id = Some("task-1".into());
    manifest.evidence.s3 = Some(eval_core::run_manifest::S3Paths {
        bucket: Some("bucket".into()),
        run_prefix: Some("runs/s/work".into()),
        trace_key: None,
        artifact_prefix: None,
    });
    store
        .save_manifest("s", "work", &run_id, &manifest)
        .unwrap();
    let html = render_portfolio(&portfolio_of(&ws_path));
    for needle in [
        "remote-service",
        "fixture:model-x",
        "free-only yes",
        "availability 98.00% (98/100 attempts)",
        "cold 120ms / 180ms",
        "queue 10ms / 20ms",
        "trace-1",
        "task-1",
        "s3://bucket/runs/s/work",
    ] {
        assert!(html.contains(needle), "missing {needle:?}");
    }
    assert!(
        !html.contains("href=\"javascript:"),
        "trace_url must not become a script link"
    );
}

#[test]
fn corrupt_manifest_is_surfaced_not_dropped() {
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let run_id = store.latest("s", "work").unwrap().unwrap();
    std::fs::write(
        dir.join("alpha/eval-runs/runs/s/work")
            .join(format!("run-{run_id}/manifest.json")),
        "{broken",
    )
    .unwrap();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(html.contains("manifest:"));
    assert!(html.contains("not compared: candidate manifest unreadable"));
    assert!(html.contains("Alpha"));
}

#[test]
fn unavailable_project_is_shown_not_dropped() {
    let (_dir, ws_path) = scaffold();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(html.contains("Beta"));
    assert!(html.contains("repository not found"));
    assert!(html.contains("unavailable"));
}

#[test]
fn corrupt_run_is_surfaced() {
    let (_dir, ws_path) = scaffold();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(html.contains("Unreadable run record"));
}

#[test]
fn regression_state_rendered() {
    let (_dir, ws_path) = scaffold();
    let html = render_portfolio(&portfolio_of(&ws_path));
    // Alpha candidate == same content as baseline: clean.
    assert!(html.contains("Clean vs baseline"));
    // Gamma has no baseline: honest claim of none.
    assert!(html.contains("No regression claim"));
}

#[test]
fn names_are_escaped() {
    let (_dir, ws_path) = scaffold();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(!html.contains("<b>project</b>"));
    assert!(html.contains("Alpha &lt;b&gt;project&lt;/b&gt;"));
}

#[test]
fn filters_and_empty_states_present() {
    let (_dir, ws_path) = scaffold();
    let html = render_portfolio(&portfolio_of(&ws_path));
    for id in [
        "f-project",
        "f-status",
        "f-suite",
        "f-lane",
        "f-since",
        "filter-count",
    ] {
        assert!(html.contains(id), "missing {id}");
    }
    assert!(html.contains("Trend (last"), "missing trend view");
}

#[test]
fn artifact_uris_encode_spaces_and_quotes() {
    use eval_graders::portfolio::*;
    let report = PortfolioReport {
        workspace_name: "ws".into(),
        generated_at_ms: 1,
        totals: PortfolioTotals::default(),
        projects: vec![ProjectReport {
            id: "p".into(),
            name: "P".into(),
            repo: ".".into(),
            available: true,
            unavailability: None,
            suites: vec![ProjectSuiteReport {
                suite_id: "s".into(),
                suite_path: "s.yaml".into(),
                available: true,
                note: None,
                refs: vec![SuiteRefReport {
                    ref_name: "work".into(),
                    run_id: "1-a".into(),
                    started_at_ms: 1,
                    code_version: "v".into(),
                    pass: true,
                    pass_rate: 1.0,
                    cases_total: 0,
                    cases_passed: 0,
                    cases_failed: 0,
                    cases_infra: 0,
                    trials_total: 0,
                    latency: None,
                    tokens: TokenTotals::default(),
                    failed_cases: Vec::new(),
                    failure_classes: Vec::new(),
                    quality: None,
                    stability: None,
                    manifest: None,
                    manifest_error: None,
                    load_error: None,
                    run_dir: "C:\\proj dir\\quote\"x".into(),
                }],
                history: Vec::new(),
                trend: Vec::new(),
                regression: None,
                regression_note: None,
            }],
            cases_total: 0,
            cases_passed: 0,
            cases_failed: 0,
            cases_infra: 0,
            last_success_ms: None,
            last_failure_ms: None,
            history: Vec::new(),
            usage: None,
        }],
    };
    let html = render_portfolio(&report);
    assert!(html.contains("proj%20dir"), "{html:.500}");
    assert!(
        !html.contains("quote\"x"),
        "raw quote must not break the attribute"
    );
}

#[test]
fn usage_sections_render_with_provenance() {
    let dir = tmpdir("usage-view");
    // Registered project with one suite run + imported usage events.
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    std::fs::write(dir.join("proj/suite.yaml"), SUITE_YAML).unwrap();
    let run_id = save(&dir.join("proj"), "s", "work", true, 1000);
    let usage = eval_usage::UsageStore::new(dir.join("proj/eval-runs"));
    let line = serde_json::json!({
        "event": "llm.call", "event_version": 1, "event_id": "u1",
        "project": "proj", "task_id": "t", "request_id": "r",
        "provider": "fixture", "model": "summarizer-a",
        "started_at_ms": 2000, "duration_ms": 40,
        "input_tokens": 100, "output_tokens": 20,
        "outcome": "ok", "origin": "synthetic", "context": "benchmark",
        "tag": "bench-a",
    });
    usage
        .import_str("proj", &serde_json::to_string(&line).unwrap(), None, None)
        .unwrap();
    usage
        .bind_tag(
            "proj",
            &eval_usage::TagBinding {
                tag: "bench-a".to_string(),
                suite: "s".to_string(),
                ref_name: "work".to_string(),
                run_id,
                imported_at_ms: 3000,
            },
        )
        .unwrap();
    std::fs::write(
        dir.join("eval-workspace.toml"),
        "[workspace]\nname = \"u\"\n\n[[project]]\nid = \"proj\"\nname = \"Proj\"\nrepo = \"proj\"\nsuites = [\"suite.yaml\"]\n",
    )
    .unwrap();
    let ws = dir.join("eval-workspace.toml");
    let file = eval_core::WorkspaceFile::load(&ws).unwrap();
    let mut report = build_portfolio(&ws, &file);
    attach_usage(&ws, &file, &mut report);
    let html = render_portfolio(&report);
    assert!(html.contains("Usage — benchmark"), "{html:.1000}");
    assert!(html.contains("summarizer-a"));
    assert!(html.contains("synthetic"));
    assert!(html.contains("Benchmark comparison"));
    assert!(html.contains("bench-a"));
}

#[test]
fn empty_workspace_renders_usefully() {
    let dir = tmpdir("empty");
    let ws_path = dir.join("eval-workspace.toml");
    std::fs::write(&ws_path, "[workspace]\nname = \"empty\"\n").unwrap();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(html.contains("empty"));
    assert!(html.contains(">0<"));
}
