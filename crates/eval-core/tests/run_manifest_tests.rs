use eval_core::run_manifest::{
    FailureClass, Lane, ModelRef, RunManifest, RunMetrics, classify_case,
};
use eval_core::*;

fn trial(case_id: &str, status: TrialStatus) -> TrialObservation {
    TrialObservation {
        case_id: case_id.into(),
        trial: 0,
        status,
        exit_code: Some(0),
        wall_ms: 10,
        output: Some(serde_json::json!({"output": {"text": "x"}})),
        events: Vec::new(),
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn case(id: &str, trials: Vec<TrialObservation>, grades: Vec<Grade>) -> CaseRecord {
    let pass = !grades.is_empty() && grades.iter().all(|g| g.pass);
    let digest = digest_trials(&trials);
    CaseRecord {
        case_id: id.into(),
        deterministic: true,
        trials,
        grades,
        pass,
        digest,
    }
}

fn grade(name: &str, pass: bool, score: Option<f64>) -> Grade {
    Grade {
        grader: name.into(),
        pass,
        score,
        evidence: "e".into(),
    }
}

fn record_with(cases: Vec<CaseRecord>) -> RunRecord {
    let trials_total = cases.iter().map(|c| c.trials.len()).sum();
    RunRecord {
        schema_version: SCHEMA_VERSION,
        run_id: String::new(),
        suite: "s".into(),
        suite_version: 1,
        code_version: "eval-1".into(),
        request_id: None,
        ref_name: "work".into(),
        started_at_ms: 1_700_000_000_000,
        target: None,
        files_root: ".".into(),
        trials_total,
        cases,
        gate: None,
    }
}

fn passing_case() -> CaseRecord {
    case(
        "ok",
        vec![trial("ok", TrialStatus::Ok)],
        vec![grade("exit_code[0]", true, None)],
    )
}

fn tempfile_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "eval-manifest-test-{tag}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn json_schema_covers_every_serialized_manifest_field() {
    let mut record = record_with(vec![passing_case()]);
    record.run_id = "1-test".into();
    let manifest = RunManifest::from_record(
        &record,
        Some(Lane::Batch),
        Some("eval-1".into()),
        Some("target-1".into()),
        Some(ModelRef {
            provider: Some("p".into()),
            model: "m".into(),
        }),
        Some(true),
    );
    let value = serde_json::to_value(&manifest).unwrap();
    let schema = eval_core::run_manifest::manifest_json_schema();
    let props = schema["properties"].as_object().unwrap();
    for key in value.as_object().unwrap().keys() {
        assert!(props.contains_key(key), "schema lacks top-level {key}");
    }
    let metric_schema = &schema["properties"]["metrics"]["properties"];
    for key in value["metrics"].as_object().unwrap().keys() {
        assert!(
            metric_schema.get(key).is_some(),
            "schema lacks metrics.{key}"
        );
    }
    for section in ["trials", "outcomes", "usage"] {
        let section_schema = &metric_schema[section]["properties"];
        for key in value["metrics"][section].as_object().unwrap().keys() {
            assert!(
                section_schema.get(key).is_some(),
                "schema lacks metrics.{section}.{key}"
            );
        }
    }
    assert_eq!(
        schema["properties"]["manifest_version"]["const"],
        serde_json::json!(eval_core::run_manifest::RUN_MANIFEST_VERSION)
    );
}

#[test]
fn failure_classes_are_one_primary_class_per_failed_case() {
    let timeout = case(
        "timeout",
        vec![trial("timeout", TrialStatus::Timeout)],
        vec![grade("exit_code[0]", false, None)],
    );
    let grader = case(
        "grader",
        vec![trial("grader", TrialStatus::Ok)],
        vec![grade("regex[output.text]", false, None)],
    );
    let record = record_with(vec![passing_case(), timeout, grader]);
    let metrics = RunMetrics::from_record(&record);
    assert_eq!(metrics.outcomes.total, Some(3));
    assert_eq!(metrics.outcomes.passed, Some(1));
    assert_eq!(metrics.outcomes.failed, Some(2));
    assert_eq!(metrics.outcomes.timeouts, Some(1));
    assert_eq!(
        metrics.failure_classes.get(&FailureClass::Timeout),
        Some(&1)
    );
    assert_eq!(metrics.failure_classes.get(&FailureClass::Grader), Some(&1));
    assert_eq!(metrics.failure_classes.values().sum::<u64>(), 2);
    metrics.validate().expect("consistent taxonomy");
    let roundtrip: RunMetrics =
        serde_json::from_value(serde_json::to_value(&metrics).unwrap()).unwrap();
    assert_eq!(roundtrip, metrics);
}

#[test]
fn failure_class_precedence_is_deterministic() {
    let both = case(
        "both",
        vec![
            trial("both", TrialStatus::Timeout),
            trial("both", TrialStatus::Crash),
        ],
        vec![grade("g", false, None)],
    );
    assert_eq!(classify_case(&both), Some(FailureClass::Crash));
    let protocol = case(
        "protocol",
        vec![trial("protocol", TrialStatus::ProtocolError)],
        vec![grade("g", false, None)],
    );
    assert_eq!(classify_case(&protocol), Some(FailureClass::ProtocolError));
    let handled = case(
        "handled",
        vec![trial("handled", TrialStatus::CaseError)],
        vec![grade("g", false, None)],
    );
    assert_eq!(classify_case(&handled), Some(FailureClass::CaseError));
    assert_eq!(classify_case(&passing_case()), None);
}

#[test]
fn missing_metrics_serialize_null_not_zero() {
    let record = record_with(vec![passing_case()]);
    let manifest = RunManifest::from_record(&record, None, None, None, None, None);
    let value = serde_json::to_value(&manifest).unwrap();
    for pointer in [
        "/metrics/usage/tokens_in",
        "/metrics/usage/tokens_out",
        "/metrics/usage/cost_usd",
        "/metrics/service",
        "/lane",
        "/free_only",
    ] {
        assert!(
            value.pointer(pointer).is_some_and(|v| v.is_null()),
            "{pointer} must be null (n/a), got {:?}",
            value.pointer(pointer)
        );
    }
    assert_eq!(manifest.metrics.outcomes.failed, Some(0));
}

#[test]
fn quality_and_stability_are_measured_or_absent() {
    let graded = case(
        "graded",
        vec![trial("graded", TrialStatus::Ok)],
        vec![
            grade("judge_lite[output.text]", true, Some(0.75)),
            grade("stability", true, None),
        ],
    );
    let metrics = RunMetrics::from_record(&record_with(vec![graded]));
    let quality = metrics.quality.expect("judge score");
    assert_eq!(quality.n, 1);
    assert_eq!(quality.mean_score, Some(0.75));
    let stability = metrics.stability.expect("stability grade");
    assert_eq!((stability.passed, stability.total), (1, 1));
}

#[test]
fn legacy_cost_is_kept_without_fabricated_provenance() {
    let mut trial = trial("usage", TrialStatus::Ok);
    trial.output = Some(serde_json::json!({
        "output": {"text": "x"},
        "ai": {"tokens_in": 11, "tokens_out": 3, "cost_usd": 0.5},
    }));
    let metrics = RunMetrics::from_record(&record_with(vec![case(
        "usage",
        vec![trial],
        vec![grade("g", true, None)],
    )]));
    assert_eq!(metrics.usage.tokens_in, Some(11));
    assert_eq!(metrics.usage.tokens_out, Some(3));
    assert_eq!(metrics.usage.cost_usd, Some(0.5));
    assert_eq!(metrics.usage.cost_provenance, None);
    assert_eq!(metrics.usage.reporting_trials, Some(1));
}

#[test]
fn validation_rejects_inconsistent_or_bad_shapes() {
    let mut record = record_with(vec![passing_case()]);
    record.run_id = "1-test".into();
    let mut manifest = RunManifest::from_record(
        &record,
        Some(Lane::Batch),
        Some("eval-1".into()),
        Some("target-1".into()),
        Some(ModelRef {
            provider: Some("p".into()),
            model: "m".into(),
        }),
        Some(true),
    );
    assert!(manifest.validate().is_ok());

    let mut wrong_version = manifest.clone();
    wrong_version.manifest_version = 99;
    assert!(wrong_version.validate().is_err());

    manifest.metrics.outcomes.failed = Some(1);
    assert!(manifest.validate().is_err(), "failed without a class");
}

#[test]
fn backfill_creates_legacy_manifests_idempotently() {
    let dir = tempfile_dir("backfill");
    let store = RunStore::new(&dir);
    for (i, ref_name) in ["baseline", "work"].iter().enumerate() {
        let mut record = record_with(vec![passing_case()]);
        record.ref_name = (*ref_name).into();
        record.started_at_ms += i as u64;
        let observations = record.cases[0].trials.clone();
        store.save(&mut record, &observations).unwrap();
    }
    let first = store.backfill_manifests().unwrap();
    assert_eq!(first.created, 2, "{first:?}");
    assert_eq!(first.existing, 0);
    assert!(first.failed.is_empty(), "{first:?}");
    for ref_name in ["baseline", "work"] {
        let id = store.latest("s", ref_name).unwrap().unwrap();
        let manifest = store
            .load_manifest("s", ref_name, &id)
            .unwrap()
            .expect("backfilled manifest");
        assert_eq!(manifest.lane, None, "backfill must not guess a lane");
        assert_eq!(manifest.free_only, None);
    }
    let second = store.backfill_manifests().unwrap();
    assert_eq!(second.created, 0);
    assert_eq!(second.existing, 2);
    assert!(second.failed.is_empty(), "{second:?}");
}

#[test]
fn store_manifest_roundtrip_and_immutability() {
    let dir = tempfile_dir("store");
    let store = RunStore::new(&dir);
    let mut record = record_with(vec![passing_case()]);
    let observations = record.cases[0].trials.clone();
    let run_id = store.save(&mut record, &observations).unwrap();
    let manifest = RunManifest::from_record(
        &record,
        Some(Lane::LocalService),
        Some("eval-1".into()),
        Some("target-1".into()),
        None,
        Some(true),
    );
    store
        .save_manifest("s", "work", &run_id, &manifest)
        .unwrap();
    let loaded = store
        .load_manifest("s", "work", &run_id)
        .unwrap()
        .expect("manifest");
    assert_eq!(loaded, manifest);

    let mut changed = manifest.clone();
    changed.free_only = Some(false);
    let err = store
        .save_manifest("s", "work", &run_id, &changed)
        .expect_err("immutable");
    assert!(err.to_string().contains("different content"), "{err}");
}
