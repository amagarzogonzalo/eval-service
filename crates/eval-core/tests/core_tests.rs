//! `eval-core` acceptance tests: config, store round-trip, redaction.

use eval_core::*;

const SUITE_YAML: &str = r#"
suite: cv-demo
version: 1
target:
  argv: ["echo"]
  mode: oneshot
env_allowlist: ["OFFLINE"]
gate:
  min_pass_rate: 1.0
cases:
  - id: validate-ok
    input:
      argv: ["validate", "resume.yaml"]
    limits: { timeout_ms: 5000, max_output_bytes: 65536 }
    trials: 1
    deterministic: true
    graders:
      - kind: exit_code
        expect: 0
      - kind: regex
        field: output.text
        pattern: "score=\\d+"
  - id: sample-case
    deterministic: false
    trials: 5
    input: { params: { n: 3 } }
    limits: { timeout_ms: 10000, max_output_bytes: 1024 }
    graders:
      - kind: stability
"#;

fn write_suite(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("suite.yaml");
    std::fs::write(&path, SUITE_YAML).unwrap();
    path
}

#[test]
fn presets_expand_into_graders() {
    let dir = tempfile_dir();
    let yaml = r##"
suite: presets
version: 1
cases:
  - id: a
    input: {}
    limits: { timeout_ms: 1000, max_output_bytes: 1024 }
    trials: 2
    deterministic: true
    presets:
      - cli_contract
      - stable
      - no_secret_leak
      - json_object: { required: [name, email] }
    graders:
      - kind: regex
        field: output.text
        pattern: "\\S"
"##;
    let path = dir.join("presets.yaml");
    std::fs::write(&path, yaml).unwrap();
    let suite = Suite::load(&path).expect("loads");
    let graders = &suite.cases[0].graders;
    // 4 presets expand to 4 graders + 1 explicit = 5.
    assert_eq!(graders.len(), 5, "{graders:?}");
    assert!(
        graders
            .iter()
            .any(|g| matches!(g, GraderSpec::ExitCode { expect: 0 }))
    );
    assert!(graders.iter().any(|g| matches!(g, GraderSpec::Stability)));
    assert!(
        graders
            .iter()
            .any(|g| matches!(g, GraderSpec::NotRegex { .. }))
    );
    assert!(
        graders
            .iter()
            .any(|g| matches!(g, GraderSpec::JsonSchema { .. }))
    );
    assert!(
        graders
            .iter()
            .any(|g| matches!(g, GraderSpec::Regex { .. }))
    );
    assert!(suite.cases[0].presets.is_empty());
}

#[test]
fn unknown_preset_is_a_loud_error() {
    let dir = tempfile_dir();
    let yaml = "suite: bad\nversion: 1\ncases:\n  - id: a\n    input: {}\n    limits: { timeout_ms: 1000, max_output_bytes: 1024 }\n    presets: [nope]\n";
    let path = dir.join("bad.yaml");
    std::fs::write(&path, yaml).unwrap();
    let err = Suite::load(&path).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("unknown grader preset"), "{msg}");
    assert!(
        msg.contains("cli_contract"),
        "must list valid presets: {msg}"
    );
}

#[test]
fn suite_loads_and_validates() {
    let dir = tempfile_dir();
    let suite = Suite::load(&write_suite(&dir)).expect("suite loads");
    assert_eq!(suite.suite, "cv-demo");
    assert_eq!(suite.cases.len(), 2);
    assert_eq!(suite.cases[0].graders.len(), 2);
}

#[test]
fn suite_rejects_duplicates_and_bad_limits() {
    let dir = tempfile_dir();
    let bad = SUITE_YAML.replace("sample-case", "validate-ok");
    let path = dir.join("bad.yaml");
    std::fs::write(&path, bad).unwrap();
    assert!(Suite::load(&path).is_err());

    let bad_timeout = SUITE_YAML.replace("timeout_ms: 5000", "timeout_ms: 0");
    let path = dir.join("bad2.yaml");
    std::fs::write(&path, bad_timeout).unwrap();
    assert!(Suite::load(&path).is_err());
}

#[test]
fn suite_rejects_sampling_with_one_trial() {
    let dir = tempfile_dir();
    let bad = SUITE_YAML.replace("trials: 5", "trials: 1");
    let path = dir.join("bad3.yaml");
    std::fs::write(&path, bad).unwrap();
    assert!(Suite::load(&path).is_err());
}

#[test]
fn timeout_rule_checked_at_load() {
    let dir = tempfile_dir();
    let with_budget = SUITE_YAML.replace(
        "target:\n  argv: [\"echo\"]",
        "target:\n  argv: [\"echo\"]\n  budget_ms: 20000",
    );
    let path = dir.join("budget.yaml");
    std::fs::write(&path, with_budget).unwrap();
    // 5000ms < 1.5 * 20000ms -> rejected.
    assert!(Suite::load(&path).is_err());
}

#[test]
fn store_round_trip() {
    let dir = tempfile_dir();
    let store = RunStore::new(dir.join("store"));
    let mut record = sample_record();
    let observations = record
        .cases
        .iter()
        .flat_map(|c| c.trials.clone())
        .collect::<Vec<_>>();
    let run_id = store.save(&mut record, &observations).expect("save");
    assert!(!run_id.is_empty());

    let loaded = store.load("cv-demo", "work", &run_id).expect("load");
    assert_eq!(loaded.run_id, run_id);
    assert_eq!(loaded.cases.len(), 1);
    assert_eq!(loaded.cases[0].trials.len(), 2);
    assert_eq!(loaded.cases[0].pass, record.cases[0].pass);

    let latest = store
        .latest("cv-demo", "work")
        .expect("latest")
        .expect("id");
    assert_eq!(latest, run_id);

    store
        .set_baseline(&BaselinePointer {
            suite: "cv-demo".to_string(),
            ref_name: "work".to_string(),
            run_id: run_id.clone(),
        })
        .expect("set baseline");
    let ptr = store.baseline("cv-demo").expect("baseline");
    assert_eq!(ptr.run_id, run_id);
}

#[test]
fn run_ids_are_stable_for_identical_content() {
    let dir = tempfile_dir();
    let store = RunStore::new(dir.join("s1"));
    let mut a = sample_record();
    let obs: Vec<_> = a.cases.iter().flat_map(|c| c.trials.clone()).collect();
    let id_a = store.save(&mut a, &obs).unwrap();

    let store2 = RunStore::new(dir.join("s2"));
    let mut b = sample_record();
    let obs2: Vec<_> = b.cases.iter().flat_map(|c| c.trials.clone()).collect();
    let id_b = store2.save(&mut b, &obs2).unwrap();
    assert_eq!(id_a, id_b);
}

#[test]
fn digests_ignore_wall_clock_noise() {
    let mut t = sample_trial("c1", 0);
    let d1 = digest_trials(std::slice::from_ref(&t));
    t.wall_ms = 999_999;
    let d2 = digest_trials(std::slice::from_ref(&t));
    assert_eq!(d1, d2);
}

#[test]
fn redaction_never_leaks_secret_shapes() {
    let cases = [
        "api_key=sk-abcdefghijklmnop123456",
        "Authorization: Bearer abcdefghij123456",
        "token: hunter2-secret-value",
        "password = \"s3cr3t!\"",
        "sk-abcdefghijklmnop",
    ];
    for secret in cases {
        let redacted = redact_string(secret);
        assert!(
            !redacted.contains("hunter2")
                && !redacted.contains("s3cr3t")
                && !redacted.contains("abcdefghij"),
            "leak in `{redacted}`"
        );
        assert!(redacted.contains(REDACTED));
        // Idempotent.
        assert_eq!(redact_string(&redacted), redacted);
    }
}

#[test]
fn redaction_applies_to_nested_json() {
    let v = serde_json::json!({"events": [{"data": {"token": "token=abc123"}}]});
    let redacted = redact_json(&v);
    let text = serde_json::to_string(&redacted).unwrap();
    assert!(!text.contains("abc123"));
}

#[test]
fn secrets_are_redacted_before_persisting() {
    let dir = tempfile_dir();
    let store = RunStore::new(dir.join("store"));
    let mut record = sample_record();
    record.cases[0].trials[0].stderr_tail = Some("oops token=supersecretvalue-here".to_string());
    let obs: Vec<_> = record.cases.iter().flat_map(|c| c.trials.clone()).collect();
    let run_id = store.save(&mut record, &obs).unwrap();
    let raw = std::fs::read_to_string(
        dir.join("store")
            .join("runs/cv-demo/work")
            .join(format!("run-{run_id}/observations.jsonl")),
    )
    .unwrap();
    assert!(!raw.contains("supersecretvalue"));
}

// --- helpers ---

fn tempfile_dir() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "eval-core-test-{}-{}-{n}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_trial(case_id: &str, trial: u32) -> TrialObservation {
    TrialObservation {
        case_id: case_id.to_string(),
        trial,
        status: TrialStatus::Ok,
        exit_code: Some(0),
        wall_ms: 12,
        output: Some(serde_json::json!({
            "type": "result",
            "case_id": case_id,
            "ok": true,
            "output": {"text": "score=71", "files": []},
            "metrics": {"wall_ms": 9},
        })),
        events: vec![serde_json::json!({"type": "event", "seq": 0})],
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn sample_record() -> RunRecord {
    let trials = vec![
        sample_trial("validate-ok", 0),
        sample_trial("validate-ok", 1),
    ];
    RunRecord {
        schema_version: 1,
        run_id: String::new(),
        suite: "cv-demo".to_string(),
        suite_version: 1,
        code_version: "dirty".to_string(),
        ref_name: "work".to_string(),
        started_at_ms: 1_700_000_000_000,
        target: Some(TargetSpec {
            argv: vec!["echo".to_string()],
            mode: TargetMode::Oneshot,
            budget_ms: None,
            cwd: None,
        }),
        files_root: ".".to_string(),
        trials_total: 2,
        cases: vec![CaseRecord {
            case_id: "validate-ok".to_string(),
            deterministic: true,
            trials: trials.clone(),
            grades: vec![Grade {
                grader: "exit_code[0]".to_string(),
                pass: true,
                score: None,
                evidence: "exit 0".to_string(),
            }],
            pass: true,
            digest: digest_trials(&trials),
        }],
        gate: None,
    }
}
