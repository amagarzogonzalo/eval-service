//! Python hygiene: every shipped Python file must byte-compile.
//! (Catches syntax defects like stray `//` comments before they reach
//! a subprocess at runtime.)

mod common;

use common::*;
use std::process::Command;

const PYTHON_FILES: &[&str] = &[
    "adapters/python_shim/main.py",
    "examples/llm_calls/provider.py",
    "examples/llm_calls/target.py",
    "integrations/cv_toolkit/adapter.py",
    "integrations/digest_agent/adapter.py",
    "integrations/digest_agent/driver.py",
    "integrations/digest_agent/usage.py",
];

#[test]
fn all_shipped_python_compiles() {
    let Some(python) = python_cmd() else {
        eprintln!("SKIP: no python interpreter");
        return;
    };
    let root = workspace_root();
    // Compile in place, then remove every __pycache__ created, so the
    // repo stays clean even on success.
    let mut created_caches = Vec::new();
    let mut failures = Vec::new();
    for file in PYTHON_FILES {
        let source = root.join(file);
        assert!(source.exists(), "shipped file missing: {file}");
        let out = Command::new(&python)
            .args(["-m", "py_compile"])
            .arg(&source)
            .output()
            .expect("spawn python");
        if !out.status.success() {
            failures.push(format!("{file}: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let cache = source.parent().unwrap().join("__pycache__");
        if cache.exists() {
            created_caches.push(cache);
        }
    }
    for cache in &created_caches {
        let _ = std::fs::remove_dir_all(cache);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // No cache dirs leak into the repo even on success.
    for file in PYTHON_FILES {
        let cache = root.join(file).parent().unwrap().join("__pycache__");
        assert!(!cache.exists(), "pycache leaked at {}", cache.display());
    }
}
