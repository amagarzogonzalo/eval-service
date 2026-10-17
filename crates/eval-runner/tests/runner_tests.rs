//! Runner unit tests that need no target binary (pure helpers +
//! validation). Live recovery tests live with the reference shim.

use eval_core::{CaseSpec, InputSpec, LimitsSpec, Suite, TargetSpec};
use eval_runner::{RunPolicy, child_env, expand_argv, run_suite};
use std::path::Path;

fn suite_with(target: TargetSpec, cases: Vec<CaseSpec>) -> Suite {
    Suite {
        suite: "s".into(),
        version: 1,
        target: Some(target),
        env_allowlist: Vec::new(),
        gate: Default::default(),
        cases,
    }
}

fn case(id: &str, timeout_ms: u64) -> CaseSpec {
    CaseSpec {
        id: id.into(),
        tags: Vec::new(),
        input: InputSpec::default(),
        limits: LimitsSpec {
            timeout_ms,
            max_output_bytes: 1024,
        },
        trials: 1,
        deterministic: true,
        presets: Vec::new(),
        graders: Vec::new(),
    }
}

#[test]
fn python_argv_expansion() {
    unsafe {
        std::env::set_var("EVAL_PYTHON", "/custom/python");
    }
    assert_eq!(
        expand_argv(&["$PYTHON".to_string(), "x.py".to_string()]),
        vec!["/custom/python".to_string(), "x.py".to_string()]
    );
    unsafe {
        std::env::remove_var("EVAL_PYTHON");
    }
    let expanded = expand_argv(&["$PYTHON".to_string()]);
    assert!(expanded[0] == "python3" || expanded[0] == "python");
    assert_eq!(
        expand_argv(&["mybin".to_string()]),
        vec!["mybin".to_string()]
    );
}

#[test]
fn child_env_allowlist_never_dumps_parent() {
    unsafe {
        std::env::set_var("EVAL_TEST_VISIBLE", "yes");
        std::env::set_var("EVAL_TEST_SECRET_XYZ", "no");
    }
    let env = child_env(&["EVAL_TEST_VISIBLE".to_string()]);
    assert_eq!(
        env.get("EVAL_TEST_VISIBLE").map(String::as_str),
        Some("yes")
    );
    assert!(!env.contains_key("EVAL_TEST_SECRET_XYZ"));
    unsafe {
        std::env::remove_var("EVAL_TEST_VISIBLE");
        std::env::remove_var("EVAL_TEST_SECRET_XYZ");
    }
}

#[test]
fn child_env_keeps_home_vars_for_real_tools() {
    // Regression test: a minimal env that drops USERPROFILE/HOME
    // breaks real tools (an NLTK-based CLI could not find its home dir
    // under the old PATH/SystemRoot/TEMP-only base set).
    let saved_profile = std::env::var("USERPROFILE").ok();
    let saved_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("USERPROFILE", "C:\\fake-home");
        std::env::set_var("HOME", "/fake-home");
    }
    let env = child_env(&[]);
    assert_eq!(
        env.get("USERPROFILE").map(String::as_str),
        Some("C:\\fake-home")
    );
    assert_eq!(env.get("HOME").map(String::as_str), Some("/fake-home"));
    unsafe {
        match saved_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
fn default_workers_are_bounded() {
    let policy = RunPolicy::default_for(Vec::new());
    assert!((1..=4).contains(&policy.workers));
}

#[tokio::test]
async fn empty_argv_is_infra_error() {
    let suite = suite_with(
        TargetSpec {
            argv: Vec::new(),
            mode: eval_core::TargetMode::Oneshot,
            budget_ms: None,
            cwd: None,
        },
        vec![case("a", 1000)],
    );
    let policy = RunPolicy::default_for(Vec::new());
    let err = run_suite(
        &suite,
        suite.target.as_ref().unwrap(),
        &policy,
        Path::new("."),
    )
    .await
    .expect_err("empty argv must fail");
    assert!(err.to_string().contains("empty"));
}

#[tokio::test]
async fn timeout_rule_violation_is_infra_error() {
    let suite = suite_with(
        TargetSpec {
            argv: vec!["nope".to_string()],
            mode: eval_core::TargetMode::Oneshot,
            budget_ms: Some(10_000),
            cwd: None,
        },
        vec![case("a", 1000)],
    );
    // Suite::validate would also reject this; the runner re-checks for
    // programmatic callers.
    let policy = RunPolicy::default_for(Vec::new());
    let err = run_suite(
        &suite,
        suite.target.as_ref().unwrap(),
        &policy,
        Path::new("."),
    )
    .await
    .expect_err("1.5x rule must fail");
    assert!(err.to_string().contains("1.5"));
}

#[tokio::test]
async fn missing_binary_becomes_protocol_observation_not_panic() {
    // A missing binary is recorded per trial (spawn failed), never a panic.
    let suite = suite_with(
        TargetSpec {
            argv: vec!["definitely-not-a-real-binary-xyz".to_string()],
            mode: eval_core::TargetMode::Oneshot,
            budget_ms: None,
            cwd: None,
        },
        vec![case("a", 2000)],
    );
    let policy = RunPolicy::default_for(Vec::new());
    let run = run_suite(
        &suite,
        suite.target.as_ref().unwrap(),
        &policy,
        Path::new("."),
    )
    .await
    .expect("run completes");
    assert_eq!(run.cases.len(), 1);
    assert_eq!(
        run.cases[0].trials[0].status,
        eval_core::TrialStatus::ProtocolError
    );
}
