//! A5 parity: Python and Rust shims produce schema-equal runs on a
//! shared fixture (byte-compare after wall-clock normalization).

mod common;

use common::*;
use eval_core::{TargetMode, TrialStatus};
use eval_runner::RunPolicy;
use std::path::Path;

fn deterministic_cases() -> Vec<eval_core::CaseSpec> {
    let mut echo = case_with("echo-01", "echo", serde_json::json!({}));
    echo.input.stdin_text = "hello parity".into();
    let mut text = case_with("text-01", "echo", serde_json::json!({"text": "score=71.2"}));
    text.input.stdin_text = "ignored".into();
    let scrub = {
        let mut c = case_with("scrub-01", "scrub", serde_json::json!({}));
        c.input.stdin_text = "leak CANARY-abc123 here".into();
        c
    };
    let error = case_with(
        "err-01",
        "error",
        serde_json::json!({"code": "no_sources", "message": "empty", "retriable": false}),
    );
    let write = case_with(
        "write-01",
        "cv_write",
        serde_json::json!({
            "write_path": "parity_out/report.json",
            "write_content": "{\"score\": 71}",
            "text": "score=71.2",
        }),
    );
    let digest = case_with("digest-01", "digest_ok", serde_json::json!({"reviews": 2}));
    vec![echo, text, scrub, error, write, digest]
}

fn normalize(run: &eval_runner::SuiteRun) -> String {
    let mut lines: Vec<String> = Vec::new();
    for case in &run.cases {
        for trial in &case.trials {
            let mut value = serde_json::json!({
                "case_id": trial.case_id,
                "status": trial.status.as_str(),
                "exit_code": trial.exit_code,
                "output": trial.output,
                "events": trial.events,
            });
            // Wall-clock noise is n/a, never compared.
            if let Some(metrics) = value
                .get_mut("output")
                .and_then(|o| o.get_mut("metrics"))
                .and_then(|m| m.as_object_mut())
            {
                metrics.remove("wall_ms");
            }
            lines.push(serde_json::to_string(&value).unwrap());
        }
    }
    lines.sort();
    lines.join("\n")
}

#[tokio::test]
async fn shim_parity_schema_equal_runs() {
    let Some(py_argv) = python_shim_argv() else {
        eprintln!("SKIP: no python interpreter");
        return;
    };
    let dir = tmpdir("parity");
    // Both shims run with the same cwd so relative write paths match.
    let _ = std::fs::create_dir_all(dir.join("parity_out"));
    let policy = RunPolicy {
        workers: 2,
        env_allowlist: Vec::new(),
    };
    let run_rust = {
        let mut suite = suite_with(
            vec![shim_bin().display().to_string()],
            TargetMode::Worker,
            deterministic_cases(),
        );
        // Run from the temp dir via target cwd override.
        suite.target.as_mut().unwrap().cwd = Some(dir.display().to_string());
        let target = suite.target.clone().unwrap();
        eval_runner::run_suite(&suite, &target, &policy, Path::new("."))
            .await
            .expect("rust shim run")
    };
    let run_py = {
        let mut suite = suite_with(py_argv, TargetMode::Worker, deterministic_cases());
        suite.target.as_mut().unwrap().cwd = Some(dir.display().to_string());
        let target = suite.target.clone().unwrap();
        eval_runner::run_suite(&suite, &target, &policy, Path::new("."))
            .await
            .expect("python shim run")
    };
    for run in [&run_rust, &run_py] {
        for case in &run.cases {
            let expected = if case.case_id == "err-01" {
                TrialStatus::CaseError
            } else {
                TrialStatus::Ok
            };
            assert_eq!(
                case.trials[0].status, expected,
                "case {} failed",
                case.case_id
            );
        }
    }
    assert_eq!(normalize(&run_rust), normalize(&run_py));
}
