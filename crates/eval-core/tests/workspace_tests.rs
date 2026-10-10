//! Workspace registry tests: loading, validation, path resolution.

use eval_core::*;
use std::path::{Path, PathBuf};

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("eval-ws-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const WORKSPACE_TOML: &str = r#"
[workspace]
name = "demo portfolio"

[[project]]
id = "alpha"
name = "Alpha"
repo = "alpha-repo"
suites = ["suites/a.yaml"]
store = "eval-runs"

[[project]]
id = "beta"
name = "Beta"
repo = "/abs/beta"
suites = []
"#;

#[test]
fn workspace_loads_and_resolves_paths() {
    let dir = tmpdir("load");
    let path = dir.join("eval-workspace.toml");
    std::fs::write(&path, WORKSPACE_TOML).unwrap();
    let file = WorkspaceFile::load(&path).expect("loads");
    assert_eq!(file.workspace.name, "demo portfolio");
    assert_eq!(file.project.len(), 2);

    let anchor = WorkspaceFile::anchor(&path);
    assert_eq!(anchor, dir);
    assert_eq!(file.project[0].repo_dir(&anchor), dir.join("alpha-repo"));
    assert_eq!(
        file.project[0].store_dir(&anchor),
        dir.join("alpha-repo/eval-runs")
    );
    assert_eq!(
        file.project[0].suite_path(&anchor, "suites/a.yaml"),
        dir.join("alpha-repo/suites/a.yaml")
    );
    // Absolute repo stays absolute (platform-native absolute path).
    let abs = std::env::temp_dir().join("eval-ws-absent-repo");
    let text = WORKSPACE_TOML.replace("/abs/beta", &abs.display().to_string().replace('\\', "/"));
    std::fs::write(&path, text).unwrap();
    let file = WorkspaceFile::load(&path).expect("loads");
    assert_eq!(file.project[1].repo_dir(&anchor), abs);
}

#[test]
fn duplicate_project_ids_rejected() {
    let dir = tmpdir("dup");
    let text = WORKSPACE_TOML.replace("id = \"beta\"", "id = \"alpha\"");
    let path = dir.join("eval-workspace.toml");
    std::fs::write(&path, text).unwrap();
    assert!(WorkspaceFile::load(&path).is_err());
}

#[test]
fn empty_ids_and_names_rejected() {
    let dir = tmpdir("empty");
    let text = WORKSPACE_TOML.replace("id = \"beta\"", "id = \"\"");
    let path = dir.join("eval-workspace.toml");
    std::fs::write(&path, text).unwrap();
    assert!(WorkspaceFile::load(&path).is_err());
}

#[test]
fn missing_file_is_not_found() {
    let err =
        WorkspaceFile::load(Path::new("/nonexistent/eval-workspace.toml")).expect_err("missing");
    assert!(matches!(err, CoreError::Io { .. } | CoreError::NotFound(_)));
}

#[test]
fn malformed_toml_rejected() {
    let dir = tmpdir("bad");
    let path = dir.join("eval-workspace.toml");
    std::fs::write(&path, "[[[not toml").unwrap();
    assert!(matches!(
        WorkspaceFile::load(&path),
        Err(CoreError::Config(_))
    ));
}
