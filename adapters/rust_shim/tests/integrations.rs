//! Real-project integrations (RC2 §2): portable suites, opt-in runs.
//!
//! These tests run the committed integration suites against the real
//! sibling repositories. They SKIP unless the documented env vars are
//! set and point at valid locations — CI never runs them.

mod common;

use common::*;
use std::path::PathBuf;
use std::process::Command;

fn env_present(vars: &[&str]) -> bool {
    vars.iter().all(|v| {
        std::env::var(v).is_ok_and(|val| {
            let expanded = val.replace('$', "");
            !val.trim().is_empty() && !expanded.trim().is_empty()
        })
    })
}

fn paths_exist(vars: &[&str]) -> bool {
    vars.iter()
        .all(|v| std::env::var(v).is_ok_and(|val| std::path::Path::new(&val).exists()))
}

fn run_suite(suite_rel: &str) -> std::process::Output {
    let root = workspace_root();
    Command::new(eval_bin())
        .current_dir(&root)
        .args([
            "run",
            &format!("integrations/{suite_rel}/suite.yaml"),
            "--ref",
            "work",
            "--store",
            "eval-runs",
        ])
        .output()
        .expect("spawn eval")
}

#[test]
fn integration_suites_contain_no_absolute_paths() {
    // Portability guard: a new developer must not need to edit
    // committed suite files. Machine locations live in env vars.
    for suite in ["cv_toolkit/suite.yaml", "digest_agent/suite.yaml"] {
        let text = std::fs::read_to_string(workspace_root().join("integrations").join(suite))
            .expect("suite readable");
        for needle in ["C:/", "C:\\", "/home/", "/Users/"] {
            assert!(
                !text.contains(needle),
                "{suite} contains machine-specific path {needle:?}"
            );
        }
    }
}

#[test]
fn real_cv_toolkit_suite() {
    const VARS: &[&str] = &["EVAL_CV_PYTHON", "EVAL_CV_SRC", "EVAL_CV_REPO"];
    if !env_present(VARS) || !paths_exist(VARS) {
        eprintln!("SKIP: set EVAL_CV_PYTHON/EVAL_CV_SRC/EVAL_CV_REPO to run the real CV suite");
        return;
    }
    let out = run_suite("cv_toolkit");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("pass rate 1.00"));
}

#[test]
fn real_digest_agent_suite() {
    const VARS: &[&str] = &["EVAL_DIGEST_PYTHON", "EVAL_DIGEST_SRC"];
    if !env_present(VARS) || !paths_exist(VARS) {
        eprintln!("SKIP: set EVAL_DIGEST_PYTHON/EVAL_DIGEST_SRC to run the real digest suite");
        return;
    }
    let out = run_suite("digest_agent");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("pass rate 1.00"));
}

const DIGEST_MOCK: &str = "{\"choices\": [{\"message\": {\"content\": \"## PR #7 - Add feature\\nLogin retries look ok in src/app.py.\\n```python\\nfor attempt in range(3):\\n    try_login()\\n```\\nRisk: retry storms.\"}}], \"usage\": {\"prompt_tokens\": 800, \"completion_tokens\": 250}}";
const JUDGE_MOCK: &str = "{\"choices\": [{\"message\": {\"content\": \"SCORE: 8\\nVERDICT: PASS\\nCRITIQUE:\\n- solid grounding\"}}], \"usage\": {\"prompt_tokens\": 300, \"completion_tokens\": 40}}";

fn stage_mock(name: &str, content: &str) -> PathBuf {
    let dir = tmpdir(name);
    let path = dir.join("mock.json");
    std::fs::write(&path, content).unwrap();
    path
}

/// Mocked live digest benchmark: full live path (guards, budgets,
/// mock transport, usage parsing, ai blocks) with zero network.
#[test]
fn digest_live_bench_mocked() {
    const VARS: &[&str] = &["EVAL_DIGEST_PYTHON", "EVAL_DIGEST_SRC"];
    if !env_present(VARS) || !paths_exist(VARS) {
        eprintln!("SKIP: set EVAL_DIGEST_PYTHON/EVAL_DIGEST_SRC for the mocked live digest test");
        return;
    }
    let dir = tmpdir("digest-live-mock");
    let mock = stage_mock("digest-live-mock", DIGEST_MOCK);
    let usage_file = dir.join("usage.jsonl");
    let store = dir.join("store").display().to_string();
    // Copy the committed live suite untouched (portability proof).
    let suite = dir.join("live_bench.yaml");
    std::fs::copy(
        workspace_root().join("integrations/digest_agent/live_bench.yaml"),
        &suite,
    )
    .unwrap();
    let out = Command::new(eval_bin())
        .current_dir(workspace_root())
        .env("EVAL_DIGEST_LIVE", "1")
        .env("EVAL_DIGEST_API_KEY", "test-key-never-sent")
        .env("EVAL_DIGEST_MODEL", "test-model")
        .env("EVAL_DIGEST_MOCK_FILE", &mock)
        .env("EVAL_USAGE_OUT", &usage_file)
        .env("EVAL_USAGE_TAG", "digest-mock-live")
        .env("EVAL_USAGE_ORIGIN", "synthetic")
        .args([
            "run",
            suite.to_str().unwrap(),
            "--ref",
            "work",
            "--store",
            &store,
        ])
        .output()
        .expect("spawn eval");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Genuine mocked usage: tokens parsed from the mock response.
    let events: Vec<serde_json::Value> = std::fs::read_to_string(&usage_file)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!events.is_empty());
    for event in &events {
        assert_eq!(event["input_tokens"], 800);
        assert_eq!(event["output_tokens"], 250);
        assert_eq!(event["origin"], "synthetic");
    }
}

/// Judge calibration offline: known-good PASSes, known-bad REVISEs.
#[test]
fn digest_judge_calibration_offline() {
    if python_cmd().is_none() {
        eprintln!("SKIP: no python interpreter");
        return;
    }
    let dir = tmpdir("judge-cal");
    let out = Command::new(eval_bin())
        .current_dir(workspace_root())
        .args([
            "run",
            "examples/llm_calls/digest_judge.yaml",
            "--ref",
            "work",
            "--store",
            dir.join("store").to_str().unwrap(),
        ])
        .output()
        .expect("spawn eval");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Mocked live judge: verdict/score parsed, judge model recorded,
// judge costs tracked under the judge model id.
#[test]
fn digest_judge_mocked_live() {
    if python_cmd().is_none() {
        eprintln!("SKIP: no python interpreter");
        return;
    }
    let dir = tmpdir("judge-mock-live");
    let mock = stage_mock("judge-mock-live", JUDGE_MOCK);
    let suite = dir.join("judge_live.yaml");
    std::fs::write(
        &suite,
        r#"suite: judge-live-smoke
version: 1
target:
  argv: ["$PYTHON", "examples/llm_calls/target.py"]
  mode: oneshot
env_allowlist: [EVAL_LLM_LIVE, EVAL_LLM_API_KEY, EVAL_LLM_MODEL, EVAL_LLM_JUDGE_MODEL, EVAL_LLM_MOCK_FILE, EVAL_USAGE_OUT]
cases:
  - id: judge-smoke
    input:
      params:
        task: digest_judge
        provider: live
        focus: accuracy
        section: |
          ## PR #9 - Fix thing
          ```python
          x = 1
          ```
          Risk noted.
    limits: { timeout_ms: 30000, max_output_bytes: 65536 }
    trials: 1
    deterministic: true
    graders:
      - kind: json_schema
        field: output.data
        schema:
          type: object
          required: [score, verdict, judge_model, rubric]
      - kind: exact
        field: output.data.verdict
        value: "PASS"
      - kind: range
        field: output.data.score
        min: 8.0
        max: 10.0
"#,
    )
    .unwrap();
    let out = Command::new(eval_bin())
        .current_dir(workspace_root())
        .env("EVAL_LLM_LIVE", "1")
        .env("EVAL_LLM_API_KEY", "test-key-never-sent")
        .env("EVAL_LLM_MODEL", "candidate-model")
        .env("EVAL_LLM_JUDGE_MODEL", "judge-model")
        .env("EVAL_LLM_MOCK_FILE", &mock)
        .args([
            "run",
            suite.to_str().unwrap(),
            "--ref",
            "work",
            "--store",
            dir.join("store").to_str().unwrap(),
        ])
        .output()
        .expect("spawn eval");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Judge identity and distinction recorded in the stored grade data.
    let store_obj = eval_core::RunStore::new(dir.join("store"));
    let id = store_obj
        .latest("judge-live-smoke", "work")
        .unwrap()
        .unwrap();
    let record = store_obj.load("judge-live-smoke", "work", &id).unwrap();
    let data = record.cases[0].trials[0].output.as_ref().unwrap()["output"]["data"].clone();
    assert_eq!(data["judge_model"], "judge-model");
    assert_eq!(data["judge_distinct"], true);
    assert_eq!(data["verdict"], "PASS");
}

