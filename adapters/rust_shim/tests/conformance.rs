//! Conformance: every target survives echo / hang / crash /
//! malformed / wrong-version / unknown-field. Recovery matrix (A4):
//! hang -> Timeout, crash -> Crash, malformed -> ProtocolError, with
//! no cross-contamination on a shared worker.

mod common;

use common::*;
use eval_core::{TargetMode, TrialStatus};
use eval_runner::RunPolicy;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

fn policy(workers: usize) -> RunPolicy {
    RunPolicy {
        workers,
        env_allowlist: Vec::new(),
        traceparent: None,
        trace_id: None,
        sandbox: None,
    }
}

async fn run(
    argv: Vec<String>,
    mode: TargetMode,
    cases: Vec<eval_core::CaseSpec>,
    workers: usize,
) -> eval_runner::SuiteRun {
    let suite = suite_with(argv, mode, cases);
    let target = suite.target.clone().unwrap();
    eval_runner::run_suite(&suite, &target, &policy(workers), Path::new("."))
        .await
        .expect("suite runs")
}

fn status_of(run: &eval_runner::SuiteRun, case_id: &str) -> TrialStatus {
    run.cases
        .iter()
        .find(|c| c.case_id == case_id)
        .expect("case present")
        .trials[0]
        .status
}

#[tokio::test]
async fn echo_happy_path_both_modes() {
    for mode in [TargetMode::Oneshot, TargetMode::Worker] {
        let mut c = case_with("echo-01", "echo", serde_json::json!({}));
        c.input.stdin_text = "hello".into();
        let run = run(vec![shim_bin().display().to_string()], mode, vec![c], 2).await;
        let trial = &run.cases[0].trials[0];
        assert_eq!(trial.status, TrialStatus::Ok);
        // Oneshot reaps the process (exit 0); worker mode keeps it alive.
        assert_eq!(trial.exit_code, (mode == TargetMode::Oneshot).then_some(0));
        let text = trial.output.as_ref().unwrap()["output"]["text"]
            .as_str()
            .unwrap();
        assert_eq!(text, "hello");
        assert!(trial.wall_ms < 10_000, "wall {}", trial.wall_ms);
    }
}

#[tokio::test]
async fn hang_becomes_timeout() {
    let mut c = case_with("hang-01", "hang", serde_json::json!({}));
    c.limits.timeout_ms = 800;
    let begun = std::time::Instant::now();
    let run = run(
        vec![shim_bin().display().to_string()],
        TargetMode::Oneshot,
        vec![c],
        1,
    )
    .await;
    assert_eq!(status_of(&run, "hang-01"), TrialStatus::Timeout);
    assert!(
        begun.elapsed().as_secs() < 20,
        "deadline must kill promptly"
    );
}

#[tokio::test]
async fn crash_reports_exit_code() {
    let run = run(
        vec![shim_bin().display().to_string()],
        TargetMode::Oneshot,
        vec![case_with("crash-01", "crash", serde_json::json!({}))],
        1,
    )
    .await;
    let trial = &run.cases[0].trials[0];
    assert_eq!(trial.status, TrialStatus::Crash);
    assert_eq!(trial.exit_code, Some(3));
    assert!(trial.error_message.as_deref().unwrap_or("").contains('3'));
}

#[tokio::test]
async fn malformed_stdout_is_protocol_error() {
    let run = run(
        vec![shim_bin().display().to_string()],
        TargetMode::Oneshot,
        vec![case_with("m-01", "malformed", serde_json::json!({}))],
        1,
    )
    .await;
    assert_eq!(status_of(&run, "m-01"), TrialStatus::ProtocolError);
}

#[tokio::test]
async fn handled_case_error_stays_gradable() {
    let run = run(
        vec![shim_bin().display().to_string()],
        TargetMode::Oneshot,
        vec![case_with(
            "e-01",
            "error",
            serde_json::json!({"code": "no_sources", "message": "empty", "retriable": false}),
        )],
        1,
    )
    .await;
    let trial = &run.cases[0].trials[0];
    assert_eq!(trial.status, TrialStatus::CaseError);
    assert_eq!(trial.exit_code, Some(0));
    assert_eq!(trial.error_code.as_deref(), Some("no_sources"));
}

#[test]
fn wrong_version_handshake_rejected() {
    let mut child = std::process::Command::new(shim_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shim");
    let bad = "{\"protocol\":\"eval/v9\",\"mode\":\"worker\"}\n";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bad.as_bytes())
        .unwrap();
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(2));
}

#[test]
fn strict_target_rejects_unknown_fields() {
    // The committed unknown-field fixture must fail a strict target.
    let fixture = workspace_root().join("protocol/conformance/unknown_field_case.json");
    let case_line = std::fs::read_to_string(fixture).expect("fixture");
    let mut child = std::process::Command::new(shim_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shim");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all("{\"protocol\":\"eval/v1\",\"mode\":\"oneshot\"}\n".as_bytes())
        .unwrap();
    stdin.write_all(case_line.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    drop(stdin);
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(2), "strict target must reject");
}

#[tokio::test]
async fn recovery_matrix_no_cross_contamination() {
    // A4: one worker handles the whole gauss of failures; every later
    // echo must still pass, proving kill+respawn isolation.
    let mut hang = case_with("hang-01", "hang", serde_json::json!({}));
    hang.limits.timeout_ms = 800;
    let mut echo_after_hang = case_with("echo-1", "echo", serde_json::json!({}));
    echo_after_hang.input.stdin_text = "after-hang".into();
    let mut echo_after_crash = case_with("echo-2", "echo", serde_json::json!({}));
    echo_after_crash.input.stdin_text = "after-crash".into();
    let mut echo_after_malformed = case_with("echo-3", "echo", serde_json::json!({}));
    echo_after_malformed.input.stdin_text = "after-malformed".into();
    let cases = vec![
        echo_after_hang,
        hang,
        echo_after_crash,
        case_with("crash-01", "crash", serde_json::json!({})),
        echo_after_malformed,
        case_with("m-01", "malformed", serde_json::json!({})),
        case_with("echo-4", "echo", serde_json::json!({})),
    ];
    let run = run(
        vec![shim_bin().display().to_string()],
        TargetMode::Worker,
        cases,
        1, // single worker forces shared-process sequencing
    )
    .await;
    let statuses: Vec<(String, TrialStatus)> = run
        .cases
        .iter()
        .map(|c| (c.case_id.clone(), c.trials[0].status))
        .collect();
    let get = |id: &str| statuses.iter().find(|(c, _)| c == id).unwrap().1;
    assert_eq!(get("echo-1"), TrialStatus::Ok);
    assert_eq!(get("hang-01"), TrialStatus::Timeout);
    assert_eq!(get("echo-2"), TrialStatus::Ok);
    assert_eq!(get("crash-01"), TrialStatus::Crash);
    assert_eq!(get("echo-3"), TrialStatus::Ok);
    assert_eq!(get("m-01"), TrialStatus::ProtocolError);
    assert_eq!(get("echo-4"), TrialStatus::Ok);
    // Payloads after failures are untainted.
    let text = |id: &str| {
        run.cases.iter().find(|c| c.case_id == id).unwrap().trials[0]
            .output
            .as_ref()
            .unwrap()["output"]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(text("echo-2"), "after-crash");
    assert_eq!(text("echo-3"), "after-malformed");
}

#[tokio::test]
async fn python_shim_matches_conformance() {
    let Some(argv) = python_shim_argv() else {
        eprintln!("SKIP: no python interpreter");
        return;
    };
    let mut c = case_with("echo-01", "echo", serde_json::json!({}));
    c.input.stdin_text = "hello".into();
    let run = run(argv, TargetMode::Oneshot, vec![c], 1).await;
    assert_eq!(status_of(&run, "echo-01"), TrialStatus::Ok);
}
