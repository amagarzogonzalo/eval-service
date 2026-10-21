//! Portfolio aggregation tests: namespacing, missing data, corrupt
//! records, regression indicators, usage precedence. Dashboard totals
//! are cross-checked in `dashboard_tests`.

use eval_core::*;
use eval_graders::portfolio::*;
use eval_usage::{EVENT_VERSION, EventOrigin, LlmEvent, Outcome, UsageContext};
use std::path::{Path, PathBuf};

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-pf-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SUITE_YAML: &str = r#"
suite: shared-name
version: 1
cases:
  - id: a
    input: {}
    limits: { timeout_ms: 1000, max_output_bytes: 1024 }
    graders: []
"#;

fn trial_ok(case_id: &str, trial: u32) -> TrialObservation {
    TrialObservation {
        case_id: case_id.into(),
        trial,
        status: TrialStatus::Ok,
        exit_code: Some(0),
        wall_ms: 10 + u64::from(trial),
        output: Some(serde_json::json!({"output": {"text": "x"}})),
        events: Vec::new(),
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn record(
    suite: &str,
    ref_name: &str,
    pass: bool,
    started: u64,
) -> (RunRecord, Vec<TrialObservation>) {
    let trial = trial_ok("a", 0);
    let obs = vec![trial.clone()];
    (
        RunRecord {
            schema_version: 1,
            run_id: String::new(),
            suite: suite.into(),
            suite_version: 1,
            code_version: "abc123".into(),
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
        },
        obs,
    )
}

/// Scaffold: workspace file + two projects sharing one suite name.
fn scaffold() -> (PathBuf, PathBuf) {
    let dir = tmpdir("scaffold");
    let ws = r#"
[workspace]
name = "test"

[[project]]
id = "alpha"
name = "Alpha"
repo = "alpha"
suites = ["suite.yaml"]

[[project]]
id = "beta"
name = "Beta"
repo = "beta"
suites = ["suite.yaml"]
"#;
    let ws_path = dir.join("eval-workspace.toml");
    std::fs::write(&ws_path, ws).unwrap();
    for project in ["alpha", "beta"] {
        std::fs::create_dir_all(dir.join(project)).unwrap();
        std::fs::write(dir.join(project).join("suite.yaml"), SUITE_YAML).unwrap();
    }
    (dir, ws_path)
}

fn save(project_dir: &Path, ref_name: &str, pass: bool, started: u64) -> String {
    let store = RunStore::new(project_dir.join("eval-runs"));
    let (mut record, obs) = record("shared-name", ref_name, pass, started);
    store.save(&mut record, &obs).unwrap()
}

fn portfolio_of(ws_path: &Path) -> PortfolioReport {
    let file = WorkspaceFile::load(ws_path).unwrap();
    build_portfolio(ws_path, &file)
}

#[test]
fn duplicate_suite_names_stay_namespaced() {
    let (dir, ws_path) = scaffold();
    save(&dir.join("alpha"), "work", true, 1000);
    save(&dir.join("beta"), "work", false, 2000);
    let report = portfolio_of(&ws_path);
    assert_eq!(report.projects.len(), 2);
    assert_eq!(report.projects[0].cases_passed, 1);
    assert_eq!(report.projects[1].cases_failed, 1);
    assert_eq!(report.totals.cases_total, 2);
    assert_eq!(report.totals.cases_passed, 1);
}

#[test]
fn discovery_finds_suites_and_skips_noise() {
    use eval_graders::portfolio::discover_suites;
    let dir = tmpdir("discover");
    for rel in [
        "eval/suite.yaml",
        "tools/x/suite.yaml",
        ".venv/lib/suite.yaml",      // skipped: hidden/venv
        "node_modules/a/suite.yaml", // skipped
        "target/deep/suite.yaml",    // skipped
    ] {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "suite: x\nversion: 1\ncases: []\n").unwrap();
    }
    let found = discover_suites(&dir, 3);
    assert_eq!(
        found,
        vec![
            "eval/suite.yaml".to_string(),
            "tools/x/suite.yaml".to_string()
        ],
        "expected only real suites, sorted, relative"
    );
}

#[test]
fn project_without_suites_discovers_them() {
    // Registering a project needs only id + name + repo when suites is
    // empty; the portfolio discovers `**/suite.yaml` under the repo.
    let dir = tmpdir("autodiscover");
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("eval")).unwrap();
    std::fs::write(repo.join("eval/suite.yaml"), SUITE_YAML).unwrap();
    std::fs::write(
        dir.join("eval-workspace.toml"),
        "[workspace]\nname = \"t\"\n\n[[project]]\nid = \"p\"\nname = \"P\"\nrepo = \"repo\"\n",
    )
    .unwrap();
    let ws = dir.join("eval-workspace.toml");
    let report = portfolio_of(&ws);
    assert_eq!(report.projects[0].suites.len(), 1);
    assert_eq!(report.projects[0].suites[0].suite_path, "eval/suite.yaml");
}

#[test]
fn missing_repo_is_listed_unavailable() {
    let (dir, ws_path) = scaffold();
    std::fs::remove_dir_all(dir.join("beta")).unwrap();
    let report = portfolio_of(&ws_path);
    let beta = report.projects.iter().find(|p| p.id == "beta").unwrap();
    assert!(!beta.available);
    assert!(
        beta.unavailability
            .as_deref()
            .unwrap()
            .contains("not found")
    );
    assert_eq!(report.totals.projects_total, 2);
    assert_eq!(report.totals.projects_available, 1);
}

#[test]
fn missing_suite_file_is_listed_not_dropped() {
    let (dir, ws_path) = scaffold();
    std::fs::remove_file(dir.join("alpha/suite.yaml")).unwrap();
    let report = portfolio_of(&ws_path);
    let suite = &report.projects[0].suites[0];
    assert!(!suite.available);
    assert!(suite.note.as_deref().unwrap().contains("unreadable"));
}

#[test]
fn corrupt_record_is_contained() {
    let (dir, ws_path) = scaffold();
    let run_id = save(&dir.join("alpha"), "work", true, 1000);
    let meta = dir
        .join("alpha/eval-runs/runs/shared-name/work")
        .join(format!("run-{run_id}/metadata.json"));
    std::fs::write(&meta, "{not json").unwrap();
    let report = portfolio_of(&ws_path);
    let entry = &report.projects[0].suites[0].refs[0];
    assert!(entry.load_error.is_some());
    // Corrupt runs contribute no cases to totals.
    assert_eq!(report.totals.cases_total, 0);
    // History still shows the run id as unreadable (not silently dropped).
    assert!(
        report.projects[0]
            .history
            .iter()
            .any(|h| h.run_id == run_id)
    );
}

#[test]
fn regression_indicator_follows_baseline() {
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let base = save(&dir.join("alpha"), "baseline", true, 1000);
    store
        .set_baseline(&BaselinePointer {
            suite: "shared-name".into(),
            ref_name: "baseline".into(),
            run_id: base,
        })
        .unwrap();
    // Identical candidate: clean.
    save(&dir.join("alpha"), "work", true, 2000);
    let report = portfolio_of(&ws_path);
    assert_eq!(report.projects[0].suites[0].regression, Some(false));

    // Failing candidate: regressed. (Newest run wins candidacy.)
    save(&dir.join("alpha"), "work", false, 3000);
    let report = portfolio_of(&ws_path);
    assert_eq!(report.projects[0].suites[0].regression, Some(true));
    assert!(
        report.projects[0].suites[0]
            .regression_note
            .as_deref()
            .unwrap()
            .contains("regressed")
    );
    // Totals count the newest run per suite only: baseline(1) + work(1)
    // must total 1 case, never 2 (double-count guard).
    assert_eq!(report.totals.cases_total, 1);
    assert_eq!(report.totals.cases_failed, 1);
    assert_eq!(report.projects[0].cases_total, 1);
}

#[test]
fn incomparable_lanes_do_not_claim_regression() {
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let base = save(&dir.join("alpha"), "baseline", true, 1000);
    store
        .set_baseline(&BaselinePointer {
            suite: "shared-name".into(),
            ref_name: "baseline".into(),
            run_id: base.clone(),
        })
        .unwrap();
    let cand = save(&dir.join("alpha"), "work", false, 2000);
    let base_record = store.load("shared-name", "baseline", &base).unwrap();
    let cand_record = store.load("shared-name", "work", &cand).unwrap();
    let base_manifest = RunManifest::from_record(
        &base_record,
        Some(Lane::Batch),
        Some("eval-1".into()),
        None,
        None,
        Some(false),
    );
    let cand_manifest = RunManifest::from_record(
        &cand_record,
        Some(Lane::RemoteService),
        Some("eval-1".into()),
        None,
        None,
        Some(false),
    );
    store
        .save_manifest("shared-name", "baseline", &base, &base_manifest)
        .unwrap();
    store
        .save_manifest("shared-name", "work", &cand, &cand_manifest)
        .unwrap();
    let report = portfolio_of(&ws_path);
    let suite = &report.projects[0].suites[0];
    assert_eq!(suite.regression, None);
    assert!(
        suite
            .regression_note
            .as_deref()
            .unwrap()
            .contains("not compared: lane batch -> remote-service"),
        "{:?}",
        suite.regression_note
    );
}

#[test]
fn trend_is_newest_first_across_refs() {
    let (dir, ws_path) = scaffold();
    for i in 0..7 {
        save(&dir.join("alpha"), "work", true, 1000 + i);
    }
    save(&dir.join("alpha"), "baseline", true, 2000);
    let report = portfolio_of(&ws_path);
    let trend = &report.projects[0].suites[0].trend;
    assert_eq!(trend.len(), 8);
    assert!(
        trend
            .windows(2)
            .all(|w| w[0].started_at_ms >= w[1].started_at_ms),
        "trend must be newest-first"
    );
    assert!(trend.iter().all(|t| t.lane.is_none()));
}

#[test]
fn no_baseline_means_no_regression_claim() {
    let (dir, ws_path) = scaffold();
    save(&dir.join("alpha"), "work", false, 1000);
    let report = portfolio_of(&ws_path);
    let suite = &report.projects[0].suites[0];
    assert_eq!(suite.regression, None);
    assert_eq!(suite.regression_note.as_deref(), Some("no baseline set"));
}

#[test]
fn deliberate_exit_pass_is_not_infra() {
    // The conventional-CLI pattern: nonzero exit graded by exit_code.
    // A passing case with crash observations is a graded pass, not
    // an infrastructure failure.
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    let mut trial = trial_ok("a", 0);
    trial.status = TrialStatus::Crash;
    trial.exit_code = Some(2);
    let obs = vec![trial.clone()];
    let mut record = RunRecord {
        schema_version: 1,
        run_id: String::new(),
        suite: "shared-name".into(),
        suite_version: 1,
        code_version: "v".into(),
        ref_name: "work".into(),
        started_at_ms: 1000,
        request_id: None,
        target: None,
        files_root: ".".into(),
        trials_total: 1,
        cases: vec![CaseRecord {
            case_id: "a".into(),
            deterministic: true,
            trials: obs.clone(),
            grades: vec![Grade {
                grader: "exit_code[2]".into(),
                pass: true,
                score: None,
                evidence: "exit 2".into(),
            }],
            pass: true,
            digest: digest_trials(&obs),
        }],
        gate: None,
    };
    store.save(&mut record, &obs).unwrap();
    assert!(!record.has_infra_failure());
    let report = portfolio_of(&ws_path);
    let entry = &report.projects[0].suites[0].refs[0];
    assert_eq!(entry.cases_infra, 0);
    assert_eq!(entry.cases_passed, 1);
}

#[test]
fn latency_and_tokens_measured_or_absent() {
    let (dir, ws_path) = scaffold();
    save(&dir.join("alpha"), "work", true, 1000);
    let report = portfolio_of(&ws_path);
    let entry = &report.projects[0].suites[0].refs[0];
    let latency = entry.latency.as_ref().expect("one trial");
    assert_eq!((latency.n, latency.min_ms, latency.p50_ms), (1, 10, 10));
    // No ai block recorded: absent, never zero.
    assert_eq!(entry.tokens.tokens_in, None);
    assert_eq!(entry.tokens.total_trials, 1);
    assert_eq!(entry.tokens.reporting_trials, 0);
}

fn usage_event(id: &str, input: u64, output: u64) -> LlmEvent {
    LlmEvent {
        event: "llm.call".to_string(),
        event_version: EVENT_VERSION,
        event_id: id.to_string(),
        project: "alpha".to_string(),
        trace_id: None,
        task_id: None,
        request_id: format!("req-{id}"),
        attempt: 0,
        provider: "fixture".to_string(),
        model: "summarizer-a".to_string(),
        started_at_ms: 2000,
        duration_ms: Some(30),
        streaming: false,
        ttft_ms: None,
        input_tokens: Some(input),
        output_tokens: Some(output),
        cached_tokens: None,
        reasoning_tokens: None,
        outcome: Outcome::Ok,
        error_code: None,
        retriable: false,
        cost_usd: None,
        cost_provenance: None,
        prompt_id: None,
        prompt_hash: None,
        run_id: None,
        case_id: None,
        tag: None,
        origin: EventOrigin::Live,
        context: UsageContext::Production,
        extensions: None,
    }
}

#[test]
fn legacy_ai_never_enters_event_totals() {
    // Precedence rule: usage views aggregate llm/v1 events ONLY, even
    // when trials also carry legacy ai blocks with different numbers.
    let (dir, ws_path) = scaffold();
    let store = RunStore::new(dir.join("alpha/eval-runs"));
    // Run record with legacy ai blocks (100 in / 50 out).
    let mut trial = trial_ok("a", 0);
    trial.output = Some(serde_json::json!({
        "output": {"text": "x"},
        "ai": {"model": "legacy", "tokens_in": 100, "tokens_out": 50},
    }));
    let obs = vec![trial.clone()];
    let mut record = RunRecord {
        schema_version: 1,
        run_id: String::new(),
        suite: "shared-name".into(),
        suite_version: 1,
        code_version: "v".into(),
        ref_name: "work".into(),
        started_at_ms: 1000,
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
                pass: true,
                score: None,
                evidence: "e".into(),
            }],
            pass: true,
            digest: digest_trials(&obs),
        }],
        gate: None,
    };
    store.save(&mut record, &obs).unwrap();
    // Usage event reporting different numbers (10 in / 5 out).
    let usage_store = eval_usage::UsageStore::new(dir.join("alpha/eval-runs"));
    let line = serde_json::to_string(&usage_event("u1", 10, 5)).unwrap();
    let summary = usage_store.import_str("alpha", &line, None, None).unwrap();
    assert_eq!(summary.accepted, 1);

    let file = WorkspaceFile::load(&ws_path).unwrap();
    let mut report = build_portfolio(&ws_path, &file);
    attach_usage(&ws_path, &file, &mut report);
    let project = report.projects.iter().find(|p| p.id == "alpha").unwrap();
    let view = project.usage.as_ref().expect("usage attached");
    assert!(view.has_legacy_ai);
    let prod = view.production.as_ref().expect("production aggregate");
    // Event numbers only: 10 in / 5 out — never 110/55.
    assert_eq!(prod.input_tokens.value, Some(10.0));
    assert_eq!(prod.output_tokens.value, Some(5.0));
    assert_eq!(prod.calls, 1);
}
