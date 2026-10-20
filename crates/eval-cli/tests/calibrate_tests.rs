//! CLI contract for `eval calibrate`: offline report, advisory default,
//! optional gating exit, and loud malformed-input failure.

use std::path::PathBuf;
use std::process::Command;

fn eval_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_eval"))
}

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "eval-calibrate-test-{tag}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn dataset(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("labels.jsonl");
    let rows = [
        r#"{"id":"a","rubric":"r/v1","judge_model":"judge/v1","human_label":"pass","judge_label":"pass","human_score":9.0,"judge_score":9.0}"#,
        r#"{"id":"b","rubric":"r/v1","judge_model":"judge/v1","human_label":"revise","judge_label":"revise","human_score":4.0,"judge_score":4.0}"#,
    ];
    std::fs::write(&path, rows.join("\n") + "\n").unwrap();
    path
}

#[test]
fn calibrate_writes_versioned_report_and_stays_advisory() {
    let dir = tmpdir("report");
    let data = dataset(&dir);
    let out = dir.join("report.json");
    let md = dir.join("report.md");
    let output = Command::new(eval_bin())
        .args([
            "calibrate",
            data.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--markdown",
            md.to_str().unwrap(),
            "--min-items",
            "1",
            "--min-position-items",
            "0",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert!(report["report_id"].as_str().unwrap().len() >= 16);
    assert_eq!(report["decision"]["advisory"], true);
    assert_eq!(report["decision"]["gating_eligible"], false);
    assert!(std::fs::read_to_string(&md).unwrap().contains("ADVISORY"));
}

#[test]
fn require_gating_fails_on_advisory() {
    let dir = tmpdir("gate");
    let data = dataset(&dir);
    let output = Command::new(eval_bin())
        .args([
            "calibrate",
            data.to_str().unwrap(),
            "--out",
            dir.join("report.json").to_str().unwrap(),
            "--require-gating",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn malformed_dataset_is_infra_error() {
    let dir = tmpdir("bad");
    let path = dir.join("bad.jsonl");
    std::fs::write(&path, "{not json}\n").unwrap();
    let output = Command::new(eval_bin())
        .args([
            "calibrate",
            path.to_str().unwrap(),
            "--out",
            dir.join("report.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
