//! Portfolio aggregation tests: namespacing, missing data, corrupt
//! records, regression indicators. Dashboard totals are cross-checked
//! in `dashboard_tests`.

use eval_core::*;
use eval_graders::portfolio::*;
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
