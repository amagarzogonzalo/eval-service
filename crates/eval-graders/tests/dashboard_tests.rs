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
        "f-since",
        "filter-count",
    ] {
        assert!(html.contains(id), "missing {id}");
    }
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
                    load_error: None,
                    run_dir: "C:\\proj dir\\quote\"x".into(),
                }],
                history: Vec::new(),
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
fn empty_workspace_renders_usefully() {
    let dir = tmpdir("empty");
    let ws_path = dir.join("eval-workspace.toml");
    std::fs::write(&ws_path, "[workspace]\nname = \"empty\"\n").unwrap();
    let html = render_portfolio(&portfolio_of(&ws_path));
    assert!(html.contains("empty"));
    assert!(html.contains(">0<"));
}
