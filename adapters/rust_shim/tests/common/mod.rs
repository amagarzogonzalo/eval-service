//! Shared test helpers: binary paths, python discovery, suite builders.

#![allow(dead_code)]

use eval_core::{CaseSpec, InputSpec, LimitsSpec, Suite, TargetMode, TargetSpec};
use std::path::PathBuf;

pub fn shim_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_eval-shim-rs"))
}

pub fn eval_bin() -> PathBuf {
    // Integration tests run from target/debug/deps/; the eval binary
    // lives one level up. Resolved at runtime (cross-package).
    let exe = std::env::current_exe().expect("current test exe");
    let name = if cfg!(windows) { "eval.exe" } else { "eval" };
    for ancestor in exe.ancestors() {
        let candidate = ancestor.join(name);
        if candidate.exists() {
            return candidate;
        }
        if ancestor.file_name().is_some_and(|n| n == "deps") {
            let candidate = ancestor.parent().unwrap_or(ancestor).join(name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    panic!("eval binary not found near {}", exe.display());
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("workspace root")
}

/// Python interpreter for the reference shim, or `None` (tests skip).
pub fn python_cmd() -> Option<String> {
    let custom = std::env::var("EVAL_PYTHON")
        .ok()
        .filter(|v| !v.trim().is_empty());
    if custom.is_some() {
        return custom;
    }
    for candidate in ["python3", "python"] {
        if std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Some(candidate.to_string());
        }
    }
    None
}

pub fn python_shim_argv() -> Option<Vec<String>> {
    let python = python_cmd()?;
    let script = workspace_root().join("adapters/python_shim/main.py");
    Some(vec![python, script.display().to_string()])
}

pub fn case_with(id: &str, behavior: &str, extra_params: serde_json::Value) -> CaseSpec {
    let mut params = serde_json::json!({"behavior": behavior});
    if let (Some(map), Value::Object(extra)) = (params.as_object_mut(), &extra_params) {
        for (k, v) in extra {
            map.insert(k.clone(), v.clone());
        }
    }
    CaseSpec {
        id: id.into(),
        tags: Vec::new(),
        input: InputSpec {
            argv: Vec::new(),
            stdin_text: String::new(),
            params: Some(params),
        },
        limits: LimitsSpec {
            timeout_ms: 5000,
            max_output_bytes: 1_048_576,
        },
        trials: 1,
        deterministic: true,
        graders: Vec::new(),
    }
}

pub fn suite_with(target_argv: Vec<String>, mode: TargetMode, cases: Vec<CaseSpec>) -> Suite {
    Suite {
        suite: "conformance".into(),
        version: 1,
        target: Some(TargetSpec {
            argv: target_argv,
            mode,
            budget_ms: None,
            cwd: None,
        }),
        env_allowlist: Vec::new(),
        gate: Default::default(),
        cases,
    }
}

pub fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-shim-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

use serde_json::Value;
