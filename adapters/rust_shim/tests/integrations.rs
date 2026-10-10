//! Real-project integrations (RC2 §2): portable suites, opt-in runs.
//!
//! These tests run the committed integration suites against the real
//! sibling repositories. They SKIP unless the documented env vars are
//! set and point at valid locations — CI never runs them.

mod common;

use common::*;
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
