//! MVP acceptance battery: A1 thin adapter, A2 gate fires, A3 gate
//! stays quiet, A7 two-project report. (A4 lives in conformance.rs,
//! A5 in parity.rs, A6 is CI-enforced hermeticity.)

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_eval(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(eval_bin())
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawn eval")
}

fn code_of(out: &std::process::Output) -> i32 {
    out.status.code().unwrap_or(99)
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Copy a committed suite into `dir`, pointing its target at the Rust
/// shim (fast, no python needed). Returns the suite path.
fn rust_backed_suite(dir: &Path, source: &Path, suite_id: &str) -> PathBuf {
    let text = std::fs::read_to_string(source).expect("read suite");
    let shim = shim_bin().display().to_string().replace('\\', "/");
    let patched = text.replace(
        "argv: [\"$PYTHON\", \"adapters/python_shim/main.py\"]",
        &format!("argv: [\"{shim}\"]"),
    );
    assert!(patched.contains(&shim), "target patched in {suite_id}");
    let path = dir.join(format!("{suite_id}.yaml"));
    std::fs::write(&path, patched).expect("write suite");
    path
}

fn workspace_datasets() -> PathBuf {
    workspace_root().join("datasets")
}

// --- A1: thin adapter, zero engine changes ---

const TOY_ADAPTER: &str = r#"#!/usr/bin/env python3
"""Third CLI tool: uppercases stdin_text. ~25 lines, no engine changes."""
import json
import sys


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if "protocol" in msg:
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
        elif msg.get("type") == "case":
            text = msg.get("input", {}).get("stdin_text", "")
            print(json.dumps({
                "type": "result", "case_id": msg["case_id"], "ok": True,
                "output": {"text": text.upper(), "files": []},
            }), flush=True)
            return


main()
"#;

const TOY_SUITE: &str = r#"
suite: toy
version: 1
target:
  argv: ["$PYTHON", "adapter.py"]
  mode: oneshot
env_allowlist: []
cases:
  - id: toy-upper
    input: { stdin_text: "hello" }
    limits: { timeout_ms: 5000, max_output_bytes: 65536 }
    graders:
      - kind: exact
        field: output.text
        value: "HELLO"
"#;

#[test]
fn a1_thin_adapter_zero_engine_changes() {
    let Some(python) = python_cmd() else {
        eprintln!("SKIP: no python interpreter");
        return;
    };
    let dir = tmpdir("a1");
    std::fs::write(dir.join("adapter.py"), TOY_ADAPTER).unwrap();
    std::fs::write(dir.join("suite.yaml"), TOY_SUITE).unwrap();
    // Adapter + suite only: 2 new files, crates/ untouched by construction
    // (this test lives outside crates/ and edits nothing in it).
    let out = run_eval(
        &dir,
        &[
            "run",
            "suite.yaml",
            "--ref",
            "work",
            "--store",
            dir.join("store").to_str().unwrap(),
        ],
    );
    let _ = python;
    assert_eq!(code_of(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout_of(&out).contains("[ok  ] toy-upper"));
}

// --- A2: intentional regression flips the gate with explanation ---

#[test]
fn a2_gate_fires_on_intentional_regression() {
    let dir = tmpdir("a2");
    let source = workspace_datasets().join("cv_demo/suite.yaml");
    let suite = rust_backed_suite(&dir, &source, "cv-demo");
    let store = dir.join("store").display().to_string();

    let baseline = run_eval(
        &dir,
        &[
            "run",
            suite.to_str().unwrap(),
            "--ref",
            "baseline",
            "--store",
            &store,
            "--set-baseline",
        ],
    );
    assert_eq!(
        code_of(&baseline),
        0,
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );

    // Intentional regression: corrupt the report content the suite writes.
    let text = std::fs::read_to_string(&suite).unwrap();
    let broken = text.replace(
        "write_content: '{\"score\": 71.2, \"passed\": true}'",
        "write_content: '{\"score\": 0}'",
    );
    assert_ne!(text, broken);
    std::fs::write(&suite, broken).unwrap();

    let candidate = run_eval(
        &dir,
        &[
            "run",
            suite.to_str().unwrap(),
            "--ref",
            "work",
            "--store",
            &store,
        ],
    );
    assert_eq!(
        code_of(&candidate),
        1,
        "candidate run must fail its graders"
    );

    let compare = run_eval(
        &dir,
        &[
            "compare",
            "--suite",
            suite.to_str().unwrap(),
            "--store",
            &store,
        ],
    );
    assert_eq!(code_of(&compare), 1);
    let stdout = stdout_of(&compare);
    assert!(stdout.contains("FAIL"), "{stdout}");
    assert!(stdout.contains("cv-validate-ok"), "{stdout}");
    assert!(stdout.contains("evidence:"), "{stdout}");
}

// --- A3: unchanged reruns stay quiet with identical digests ---

#[test]
fn a3_rerun_is_quiet_with_identical_digest() {
    let dir = tmpdir("a3");
    let source = workspace_datasets().join("cv_demo/suite.yaml");
    let suite = rust_backed_suite(&dir, &source, "cv-demo");
    let store = dir.join("store").display().to_string();

    for ref_name in ["run1", "run2"] {
        let out = run_eval(
            &dir,
            &[
                "run",
                suite.to_str().unwrap(),
                "--ref",
                ref_name,
                "--store",
                &store,
            ],
        );
        assert_eq!(code_of(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    }
    let compare = run_eval(
        &dir,
        &[
            "compare",
            "--suite",
            suite.to_str().unwrap(),
            "--store",
            &store,
            "--baseline-ref",
            "run1",
            "--candidate-ref",
            "run2",
        ],
    );
    assert_eq!(code_of(&compare), 0, "{}", stdout_of(&compare));
    assert!(stdout_of(&compare).contains("PASS"));

    let store_obj = eval_core::RunStore::new(dir.join("store"));
    let id1 = store_obj.latest("cv-demo", "run1").unwrap().unwrap();
    let id2 = store_obj.latest("cv-demo", "run2").unwrap().unwrap();
    let r1 = store_obj.load("cv-demo", "run1", &id1).unwrap();
    let r2 = store_obj.load("cv-demo", "run2", &id2).unwrap();
    assert_eq!(
        eval_core::digest_run(&r1.cases),
        eval_core::digest_run(&r2.cases)
    );
}

// --- WS4: LLM examples run offline; live is explicit opt-in ---

fn stage_llm_examples(dir: &Path) -> PathBuf {
    let src = workspace_root().join("examples/llm_calls");
    let dst = dir.join("examples/llm_calls");
    std::fs::create_dir_all(&dst).unwrap();
    for file in [
        "target.py",
        "provider.py",
        "extract.yaml",
        "classify.yaml",
        "summarize.yaml",
        "live_smoke.yaml",
    ] {
        std::fs::copy(src.join(file), dst.join(file)).unwrap();
    }
    dst
}

#[test]
fn llm_examples_offline() {
    let dir = tmpdir("llm-offline");
    // Suites reference examples/llm_calls/target.py, so run from the
    // staged root that mirrors the repository layout.
    let root = dir.join("root");
    std::fs::create_dir_all(root.join("examples")).unwrap();
    let src = workspace_root().join("examples/llm_calls");
    let dst = root.join("examples/llm_calls");
    std::fs::create_dir_all(&dst).unwrap();
    for file in [
        "target.py",
        "provider.py",
        "extract.yaml",
        "classify.yaml",
        "summarize.yaml",
    ] {
        std::fs::copy(src.join(file), dst.join(file)).unwrap();
    }
    let store = root.join("store").display().to_string();
    for suite in ["extract.yaml", "classify.yaml", "summarize.yaml"] {
        let path = dst.join(suite);
        let out = Command::new(eval_bin())
            .current_dir(&root)
            .args([
                "run",
                path.to_str().unwrap(),
                "--ref",
                "work",
                "--store",
                &store,
            ])
            .output()
            .expect("spawn eval");
        assert_eq!(
            code_of(&out),
            0,
            "{suite}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Offline runs record no ai block: absent means n/a, never zero.
    let store_obj = eval_core::RunStore::new(root.join("store"));
    for suite in ["llm-extract", "llm-classify", "llm-summarize"] {
        let id = store_obj.latest(suite, "work").unwrap().unwrap();
        let record = store_obj.load(suite, "work", &id).unwrap();
        for case in &record.cases {
            for trial in &case.trials {
                let has_ai = trial.output.as_ref().is_some_and(|o| o.get("ai").is_some());
                assert!(!has_ai, "{suite}/{} has ai block offline", case.case_id);
            }
        }
    }
}

#[test]
fn llm_live_smoke_requires_explicit_opt_in() {
    // Without EVAL_LLM_LIVE=1 the live suite must fail as case_error
    // (explicit), never silently call a provider.
    let dir = tmpdir("llm-live-guard");
    let examples = stage_llm_examples(&dir);
    let path = examples.join("live_smoke.yaml");
    let store = dir.join("store").display().to_string();
    let out = Command::new(eval_bin())
        .current_dir(&dir)
        .env("EVAL_LLM_LIVE", "0")
        .env_remove("EVAL_LLM_API_KEY")
        .args([
            "run",
            path.to_str().unwrap(),
            "--ref",
            "work",
            "--store",
            &store,
        ])
        .output()
        .expect("spawn eval");
    assert_eq!(
        code_of(&out),
        1,
        "live suite without opt-in must fail graded"
    );
    let store_obj = eval_core::RunStore::new(dir.join("store"));
    let id = store_obj.latest("llm-live-smoke", "work").unwrap().unwrap();
    let record = store_obj.load("llm-live-smoke", "work", &id).unwrap();
    for case in &record.cases {
        assert_eq!(case.trials[0].status, eval_core::TrialStatus::CaseError);
        assert_eq!(
            case.trials[0].error_code.as_deref(),
            Some("live_not_enabled")
        );
    }
}

#[test]
fn llm_live_smoke_opt_in() {
    // Real provider calls ONLY with explicit opt-in + credentials.
    // Not run in CI. Reports honestly when credentials are absent.
    let live = std::env::var("EVAL_LLM_LIVE").as_deref() == Ok("1");
    let key = std::env::var("EVAL_LLM_API_KEY").unwrap_or_default();
    if !live || key.trim().is_empty() {
        eprintln!("SKIP: live inference not enabled (EVAL_LLM_LIVE=1 + EVAL_LLM_API_KEY required)");
        return;
    }
    let dir = tmpdir("llm-live");
    let examples = stage_llm_examples(&dir);
    let path = examples.join("live_smoke.yaml");
    let store = dir.join("store").display().to_string();
    let out = Command::new(eval_bin())
        .current_dir(&dir)
        .args([
            "run",
            path.to_str().unwrap(),
            "--ref",
            "live",
            "--store",
            &store,
        ])
        .output()
        .expect("spawn eval");
    // Live output varies: accept pass or graded fail, never infra crash.
    assert!(
        code_of(&out) == 0 || code_of(&out) == 1,
        "live smoke must not crash the harness: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("live smoke exit: {}", code_of(&out));
}

// --- Committed suites run as-shipped with the Python shim ---

#[test]
fn committed_suites_run_with_python_shim() {
    let Some(python) = python_cmd() else {
        eprintln!("SKIP: no python interpreter");
        return;
    };
    let dir = tmpdir("committed");
    // Committed suites reference the shim by repo-relative path; mirror
    // that layout into the temp dir so runs stay hermetic.
    let shim_src = workspace_root().join("adapters/python_shim/main.py");
    let shim_dst = dir.join("adapters/python_shim/main.py");
    std::fs::create_dir_all(shim_dst.parent().unwrap()).unwrap();
    std::fs::copy(&shim_src, &shim_dst).unwrap();
    for suite_name in [
        "cv_demo/suite.yaml",
        "digest_demo/suite.yaml",
        "security/suite.yaml",
    ] {
        let source = workspace_datasets().join(suite_name);
        let target = dir.join(suite_name.replace('/', "-"));
        std::fs::copy(&source, &target).unwrap();
        let out = Command::new(eval_bin())
            .current_dir(&dir)
            .env("EVAL_PYTHON", &python)
            .args([
                "run",
                target.to_str().unwrap(),
                "--ref",
                "work",
                "--store",
                dir.join("store").to_str().unwrap(),
            ])
            .output()
            .expect("spawn eval");
        assert_eq!(
            code_of(&out),
            0,
            "{suite_name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// --- A7: one report renders both suites ---

#[test]
fn a7_two_project_report() {
    let dir = tmpdir("a7");
    let cv = rust_backed_suite(
        &dir,
        &workspace_datasets().join("cv_demo/suite.yaml"),
        "cv-demo",
    );
    let digest = rust_backed_suite(
        &dir,
        &workspace_datasets().join("digest_demo/suite.yaml"),
        "digest-demo",
    );
    let store = dir.join("store").display().to_string();

    for suite in [&cv, &digest] {
        let out = run_eval(
            &dir,
            &[
                "run",
                suite.to_str().unwrap(),
                "--ref",
                "work",
                "--store",
                &store,
            ],
        );
        assert_eq!(
            code_of(&out),
            0,
            "{}: {}",
            suite.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let report = run_eval(
        &dir,
        &[
            "report",
            "--store",
            &store,
            "--suite",
            "cv-demo",
            "--suite",
            "digest-demo",
        ],
    );
    assert_eq!(
        code_of(&report),
        0,
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    let stdout = stdout_of(&report);
    assert!(stdout.contains("suite cv-demo"), "{stdout}");
    assert!(stdout.contains("suite digest-demo"), "{stdout}");
    // Deterministic-vs-sampling handling: sampled case ran 5 trials.
    let store_obj = eval_core::RunStore::new(dir.join("store"));
    let id = store_obj.latest("digest-demo", "work").unwrap().unwrap();
    let record = store_obj.load("digest-demo", "work", &id).unwrap();
    let sampled = record
        .cases
        .iter()
        .find(|c| c.case_id == "digest-sampled-summaries")
        .unwrap();
    assert_eq!(sampled.trials.len(), 5);
    assert!(!sampled.deterministic);
}
