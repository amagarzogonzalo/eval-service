//! CLI tests without live targets: `init`, `compare`, `report`,
//! `projects`, `dashboard`, exit-code contract. Live `run` coverage
//! lives in the shim battery.

use eval_core::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn eval_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_eval"))
}

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-cli-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn trial_ok(case_id: &str, trial: u32, text: &str) -> TrialObservation {
    TrialObservation {
        case_id: case_id.into(),
        trial,
        status: TrialStatus::Ok,
        exit_code: Some(0),
        wall_ms: 5,
        output: Some(serde_json::json!({"output": {"text": text}})),
        events: Vec::new(),
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn passing_case(id: &str, text: &str, deterministic: bool) -> CaseRecord {
    let trials = vec![trial_ok(id, 0, text)];
    CaseRecord {
        case_id: id.into(),
        deterministic,
        trials: trials.clone(),
        grades: vec![Grade {
            grader: "exact[output.text]".into(),
            pass: true,
            score: Some(1.0),
            evidence: "match".into(),
        }],
        pass: true,
        digest: digest_trials(&trials),
    }
}

fn record(
    suite: &str,
    ref_name: &str,
    cases: Vec<CaseRecord>,
) -> (RunRecord, Vec<TrialObservation>) {
    let obs: Vec<_> = cases.iter().flat_map(|c| c.trials.clone()).collect();
    (
        RunRecord {
            schema_version: 1,
            run_id: String::new(),
            suite: suite.into(),
            suite_version: 1,
            code_version: "dirty".into(),
            ref_name: ref_name.into(),
            started_at_ms: 1_700_000_000_000,
            target: None,
            files_root: ".".into(),
            trials_total: obs.len(),
            cases,
            gate: None,
        },
        obs,
    )
}

const SUITE_YAML: &str = r#"
suite: demo
version: 1
cases:
  - id: a
    input: {}
    limits: { timeout_ms: 1000, max_output_bytes: 1024 }
    graders: []
"#;

fn write_suite(dir: &Path) -> PathBuf {
    let p = dir.join("suite.yaml");
    std::fs::write(&p, SUITE_YAML).unwrap();
    p
}

fn save_run(store_root: &Path, suite: &str, ref_name: &str, cases: Vec<CaseRecord>) -> String {
    let store = RunStore::new(store_root);
    let (mut record, obs) = record(suite, ref_name, cases);
    store.save(&mut record, &obs).unwrap()
}

#[test]
fn version_carries_build_stamp() {
    // Diagnostic for stale-binary questions: every build stamps its
    // own unix time into --version.
    let out = Command::new(eval_bin())
        .arg("--version")
        .output()
        .expect("run --version");
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("eval 0.1.0 (build "), "{text}");
    assert!(text.trim_end().ends_with(')'), "{text}");
}

#[test]
fn init_scaffolds_adapter_and_suite() {
    let dir = tmpdir("init");
    let out = Command::new(eval_bin())
        .args(["init", "toy", "--dir"])
        .arg(&dir)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(dir.join("toy/adapter/target.py").exists());
    assert!(dir.join("toy/suite.yaml").exists());
    // The scaffolded suite parses.
    assert!(Suite::load(&dir.join("toy/suite.yaml")).is_ok());
}

#[test]
fn init_templates_are_runnable_and_check_clean() {
    // Guard against template rot: every shipped template must scaffold,
    // validate, and run green with zero edits — the promise an agent
    // relies on to start from a passing state.
    for template in ["cli", "llm", "agent"] {
        let dir = tmpdir(&format!("init-{template}"));
        let out = Command::new(eval_bin())
            .args(["init", "demo", "--template", template, "--dir"])
            .arg(&dir)
            .output()
            .expect("run init");
        assert_eq!(out.status.code(), Some(0), "{template}: {out:?}");
        let project = dir.join("demo");
        assert!(project.join("adapter/target.py").exists(), "{template}");

        let check = Command::new(eval_bin())
            .current_dir(&project)
            .args(["check", "suite.yaml", "--probe"])
            .output()
            .expect("run check");
        assert_eq!(
            check.status.code(),
            Some(0),
            "{template} check failed: {}",
            String::from_utf8_lossy(&check.stdout)
        );

        let run = Command::new(eval_bin())
            .current_dir(&project)
            .args([
                "run",
                "suite.yaml",
                "--ref",
                "baseline",
                "--store",
                "./store",
            ])
            .output()
            .expect("run suite");
        assert_eq!(
            run.status.code(),
            Some(0),
            "{template} run failed: {}",
            String::from_utf8_lossy(&run.stdout)
        );
    }
}

#[test]
fn init_rejects_unknown_template() {
    let dir = tmpdir("init-bad");
    let out = Command::new(eval_bin())
        .args(["init", "x", "--template", "nope", "--dir"])
        .arg(&dir)
        .output()
        .expect("run init");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn onboard_scaffolds_checks_and_sets_baseline() {
    // One command from zero to a verified baseline, run from a directory
    // that is NOT the project (onboard resolves paths itself).
    let dir = tmpdir("onboard");
    let store = dir.join("store");
    let out = Command::new(eval_bin())
        .args(["onboard", "demo", "--template", "cli", "--dir"])
        .arg(&dir)
        .args(["--store", store.to_str().unwrap()])
        .output()
        .expect("run onboard");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK — suite is valid"), "{stdout}");
    assert!(stdout.contains("baseline:"), "{stdout}");
    // A baseline pointer now exists for the scaffolded suite.
    let ptr = eval_core::RunStore::new(&store)
        .baseline("demo")
        .expect("baseline set");
    assert!(!ptr.run_id.is_empty());
}

#[test]
fn onboard_reports_failure_without_baseline() {
    // Onboard must stop at the first failure rather than set a bad
    // baseline: an unknown template exits non-zero.
    let dir = tmpdir("onboard-bad");
    let out = Command::new(eval_bin())
        .args(["onboard", "x", "--template", "nope", "--dir"])
        .arg(&dir)
        .output()
        .expect("run onboard");
    assert_ne!(out.status.code(), Some(0));
    assert!(!dir.join("x/suite.yaml").exists());
}

#[test]
fn compare_passes_on_identical_runs() {
    let dir = tmpdir("compare-pass");
    let suite = write_suite(&dir);
    let store = dir.join("store");
    save_run(
        &store,
        "demo",
        "baseline",
        vec![passing_case("a", "x", true)],
    );
    save_run(&store, "demo", "work", vec![passing_case("a", "x", true)]);
    let out = Command::new(eval_bin())
        .args([
            "compare",
            "--suite",
            suite.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
        ])
        .output()
        .expect("run compare");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("PASS"));
}

#[test]
fn compare_fails_with_named_case_and_evidence() {
    let dir = tmpdir("compare-fail");
    let suite = write_suite(&dir);
    let store = dir.join("store");
    save_run(
        &store,
        "demo",
        "baseline",
        vec![passing_case("a", "score=71", true)],
    );
    save_run(
        &store,
        "demo",
        "work",
        vec![passing_case("a", "score=12", true)],
    );
    let out = Command::new(eval_bin())
        .args([
            "compare",
            "--suite",
            suite.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
        ])
        .output()
        .expect("run compare");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FAIL") && stdout.contains("`a`"),
        "{stdout}"
    );
    assert!(stdout.contains("evidence:"), "{stdout}");
}

#[test]
fn compare_json_is_machine_readable() {
    let dir = tmpdir("compare-json");
    let suite = write_suite(&dir);
    let store = dir.join("store");
    save_run(
        &store,
        "demo",
        "baseline",
        vec![passing_case("a", "x", true)],
    );
    save_run(&store, "demo", "work", vec![passing_case("a", "x", true)]);
    let out = Command::new(eval_bin())
        .args([
            "compare",
            "--suite",
            suite.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run compare");
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["pass"], true);
}

#[test]
fn missing_suite_is_exit_2() {
    let out = Command::new(eval_bin())
        .args(["compare", "--suite", "nope.yaml"])
        .output()
        .expect("run compare");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn report_renders_multiple_suites() {
    let dir = tmpdir("report");
    let store = dir.join("store");
    save_run(&store, "one", "work", vec![passing_case("a", "x", true)]);
    save_run(&store, "two", "work", vec![passing_case("b", "y", true)]);
    let out = Command::new(eval_bin())
        .args([
            "report",
            "--store",
            store.to_str().unwrap(),
            "--suite",
            "one",
            "--suite",
            "two",
        ])
        .output()
        .expect("run report");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("suite one") && stdout.contains("suite two"),
        "{stdout}"
    );
}

fn write_workspace(dir: &Path) -> PathBuf {
    let ws = dir.join("eval-workspace.toml");
    std::fs::write(
        &ws,
        r#"
[workspace]
name = "cli-test"

[[project]]
id = "demo"
name = "Demo"
repo = "."
suites = ["suite.yaml"]
store = "store"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("suite.yaml"),
        "suite: demo\nversion: 1\ncases:\n  - id: a\n    input: {}\n    limits: { timeout_ms: 1000, max_output_bytes: 1024 }\n    graders: []\n",
    )
    .unwrap();
    ws
}

fn save_demo_run(dir: &Path, ref_name: &str) {
    let store = RunStore::new(dir.join("store"));
    let (mut record, obs) = record("demo", ref_name, vec![passing_case("a", "x", true)]);
    store.save(&mut record, &obs).unwrap();
}

#[test]
fn projects_lists_registered_with_status() {
    let dir = tmpdir("projects");
    let ws = write_workspace(&dir);
    save_demo_run(&dir, "work");
    let out = Command::new(eval_bin())
        .args(["projects", "--workspace", ws.to_str().unwrap()])
        .output()
        .expect("run projects");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Demo") && stdout.contains("[ok]"),
        "{stdout}"
    );
}

#[test]
fn projects_json_is_machine_readable() {
    let dir = tmpdir("projects-json");
    let ws = write_workspace(&dir);
    save_demo_run(&dir, "work");
    let out = Command::new(eval_bin())
        .args(["projects", "--workspace", ws.to_str().unwrap(), "--json"])
        .output()
        .expect("run projects");
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["projects"][0]["id"], "demo");
    assert_eq!(v["totals"]["cases_passed"], 1);
}

#[test]
fn projects_shows_missing_repo() {
    let dir = tmpdir("projects-missing");
    let ws = dir.join("eval-workspace.toml");
    std::fs::write(
        &ws,
        "[workspace]\nname = \"t\"\n\n[[project]]\nid = \"gone\"\nname = \"Gone\"\nrepo = \"nope\"\nsuites = []\n",
    )
    .unwrap();
    let out = Command::new(eval_bin())
        .args(["projects", "--workspace", ws.to_str().unwrap()])
        .output()
        .expect("run projects");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("unavailable"));
}

#[test]
fn dashboard_writes_index_html() {
    let dir = tmpdir("dashboard");
    let ws = write_workspace(&dir);
    save_demo_run(&dir, "work");
    let out_dir = dir.join("site");
    let out = Command::new(eval_bin())
        .args([
            "dashboard",
            "--workspace",
            ws.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("run dashboard");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let html = std::fs::read_to_string(out_dir.join("index.html")).expect("index.html");
    assert!(html.contains("Demo") && html.contains("demo"));
    assert!(html.contains("f-project"));
}

#[test]
fn dashboard_missing_workspace_is_exit_2() {
    let out = Command::new(eval_bin())
        .args(["dashboard", "--workspace", "nope.toml"])
        .output()
        .expect("run dashboard");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}
