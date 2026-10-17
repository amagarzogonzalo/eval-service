//! Grader, statistics, and gate acceptance tests.

use eval_core::*;
use eval_graders::*;
use std::path::Path;

fn trial_ok(case_id: &str, text: &str) -> TrialObservation {
    TrialObservation {
        case_id: case_id.to_string(),
        trial: 0,
        status: TrialStatus::Ok,
        exit_code: Some(0),
        wall_ms: 10,
        output: Some(serde_json::json!({
            "type": "result",
            "case_id": case_id,
            "ok": true,
            "output": {"text": text, "files": []},
            "metrics": {"wall_ms": 8},
        })),
        events: vec![serde_json::json!({"type": "event", "seq": 0, "name": "stage_done"})],
        stderr_tail: None,
        error_code: None,
        error_message: None,
    }
}

fn case_spec(id: &str, graders: Vec<GraderSpec>) -> CaseSpec {
    CaseSpec {
        id: id.to_string(),
        tags: Vec::new(),
        input: InputSpec::default(),
        limits: LimitsSpec {
            timeout_ms: 1000,
            max_output_bytes: 1024,
        },
        trials: 1,
        deterministic: true,
        presets: Vec::new(),
        graders,
    }
}

fn grade(spec: &GraderSpec, trial: &TrialObservation) -> Grade {
    grade_one(spec, trial, Path::new("."))
}

// --- deterministic graders ---

#[test]
fn exit_code_grader() {
    let t = trial_ok("a", "x");
    assert!(grade(&GraderSpec::ExitCode { expect: 0 }, &t).pass);
    assert!(!grade(&GraderSpec::ExitCode { expect: 1 }, &t).pass);
    let mut crashed = t.clone();
    crashed.exit_code = None;
    crashed.status = TrialStatus::Crash;
    assert!(!grade(&GraderSpec::ExitCode { expect: 0 }, &crashed).pass);
}

#[test]
fn exact_regex_and_not_regex() {
    let t = trial_ok("a", "score=71.2");
    assert!(
        grade(
            &GraderSpec::Exact {
                field: "output.text".into(),
                value: "score=71.2".into()
            },
            &t
        )
        .pass
    );
    assert!(
        !grade(
            &GraderSpec::Exact {
                field: "output.text".into(),
                value: "score=0".into()
            },
            &t
        )
        .pass
    );
    assert!(
        grade(
            &GraderSpec::Regex {
                field: "output.text".into(),
                pattern: r"score=\d+".into()
            },
            &t
        )
        .pass
    );
    assert!(
        !grade(
            &GraderSpec::Regex {
                field: "output.text".into(),
                pattern: r"score=\d+(\.\d+)?!".into()
            },
            &t
        )
        .pass
    );
    // Canary must never leak: forbidden pattern present -> fail.
    let mut leaked = t.clone();
    leaked.output = Some(serde_json::json!({"output": {"text": "CANARY-abc123 inside"}}));
    assert!(
        !grade(
            &GraderSpec::NotRegex {
                field: "output.text".into(),
                pattern: "CANARY-abc123".into()
            },
            &leaked
        )
        .pass
    );
    assert!(
        grade(
            &GraderSpec::NotRegex {
                field: "output.text".into(),
                pattern: "CANARY-abc123".into()
            },
            &t
        )
        .pass
    );
    // Bad patterns fail loudly, never silently pass.
    assert!(
        !grade(
            &GraderSpec::Regex {
                field: "output.text".into(),
                pattern: "([invalid".into()
            },
            &t
        )
        .pass
    );
}

#[test]
fn range_grader_bounds() {
    let t = trial_ok("a", "x");
    assert!(
        grade(
            &GraderSpec::Range {
                field: "metrics.wall_ms".into(),
                min: Some(0.0),
                max: Some(100.0)
            },
            &t
        )
        .pass
    );
    assert!(
        !grade(
            &GraderSpec::Range {
                field: "metrics.wall_ms".into(),
                min: Some(50.0),
                max: None
            },
            &t
        )
        .pass
    );
    // Non-numeric field fails, never vacuously passes.
    assert!(
        !grade(
            &GraderSpec::Range {
                field: "output.text".into(),
                min: None,
                max: None
            },
            &t
        )
        .pass
    );
}

#[test]
fn json_schema_subset() {
    let t = trial_ok("a", "x");
    let schema = serde_json::json!({
        "type": "object",
        "required": ["text", "files"],
        "properties": {"text": {"type": "string"}}
    });
    assert!(
        grade(
            &GraderSpec::JsonSchema {
                field: Some("output.output".into()),
                schema: schema.clone()
            },
            &t
        )
        .pass
    );
    let bad_schema = serde_json::json!({
        "type": "object",
        "required": ["missing_key"],
    });
    let g = grade(
        &GraderSpec::JsonSchema {
            field: Some("output.output".into()),
            schema: bad_schema,
        },
        &t,
    );
    assert!(!g.pass);
    assert!(g.evidence.contains("missing required"));
}

#[test]
fn absent_fields_never_pass() {
    let mut t = trial_ok("a", "x");
    t.output = None;
    for spec in [
        GraderSpec::Exact {
            field: "output.text".into(),
            value: "x".into(),
        },
        GraderSpec::Regex {
            field: "output.text".into(),
            pattern: "x".into(),
        },
        GraderSpec::NotRegex {
            field: "output.text".into(),
            pattern: "x".into(),
        },
        GraderSpec::Range {
            field: "metrics.wall_ms".into(),
            min: None,
            max: None,
        },
    ] {
        assert!(!grade(&spec, &t).pass, "{}", spec.name());
    }
}

#[test]
fn stability_needs_two_trials_and_detects_drift() {
    let spec = case_spec("s", vec![GraderSpec::Stability]);
    let one = vec![trial_ok("s", "same")];
    let g = grade_case(&spec, &one, Path::new("."));
    assert!(g[0].pass, "single trial is n/a, not failure");

    let two_same = vec![trial_ok("s", "same"), trial_ok("s", "same")];
    let g = grade_case(&spec, &two_same, Path::new("."));
    assert!(g[0].pass);

    let drifted = vec![trial_ok("s", "same"), trial_ok("s", "different")];
    let g = grade_case(&spec, &drifted, Path::new("."));
    assert!(!g[0].pass);
    assert!(g[0].evidence.contains("differs"));
}

#[test]
fn stability_rejects_infrastructure_observations() {
    // Two identical crashes are not "stable": infra observations can
    // never satisfy stability, or a dead target would pass vacuously.
    let spec = case_spec("s", vec![GraderSpec::Stability]);
    let mut crashed = trial_ok("s", "same");
    crashed.status = TrialStatus::Crash;
    crashed.exit_code = Some(3);
    crashed.output = None;
    let two_crashes = vec![crashed.clone(), crashed];
    let g = grade_case(&spec, &two_crashes, Path::new("."));
    assert!(!g[0].pass, "crashes must not be stable");
    assert!(g[0].evidence.contains("never stable"));

    // Handled domain negatives stay comparable.
    let mut err = trial_ok("s", "same");
    err.status = TrialStatus::CaseError;
    let two_errors = vec![err.clone(), err];
    let g = grade_case(&spec, &two_errors, Path::new("."));
    assert!(g[0].pass);
}

#[test]
fn judge_lite_scores_citations_and_length() {
    let good = trial_ok(
        "a",
        &"Findings: latency down [1], budget intact [2]. ".repeat(10),
    );
    let g = grade(
        &GraderSpec::JudgeLite {
            field: "output.text".into(),
            min_score: 0.5,
        },
        &good,
    );
    assert!(g.pass, "{}", g.evidence);
    assert!(g.score.unwrap() >= 0.5);

    let weak = trial_ok("a", "pending");
    let g = grade(
        &GraderSpec::JudgeLite {
            field: "output.text".into(),
            min_score: 0.5,
        },
        &weak,
    );
    assert!(!g.pass, "{}", g.evidence);

    // Threshold zero tracks without binding (sampling visibility).
    let g = grade(
        &GraderSpec::JudgeLite {
            field: "output.text".into(),
            min_score: 0.0,
        },
        &weak,
    );
    assert!(g.pass);
}

#[test]
fn file_graders() {
    let dir = std::env::temp_dir().join(format!(
        "eval-graders-{}-{}",
        std::process::id(),
        eval_core::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.json"), r#"{"score": 71}"#).unwrap();
    let t = trial_ok("a", "x");
    assert!(
        grade_one(
            &GraderSpec::FileExists {
                path: "report.json".into()
            },
            &t,
            &dir
        )
        .pass
    );
    assert!(
        grade_one(
            &GraderSpec::FileMatches {
                path: "report.json".into(),
                pattern: r#""score":\s*71"#.into()
            },
            &t,
            &dir
        )
        .pass
    );
    assert!(
        !grade_one(
            &GraderSpec::FileMatches {
                path: "report.json".into(),
                pattern: r#""score":\s*0"#.into()
            },
            &t,
            &dir
        )
        .pass
    );
    assert!(
        !grade_one(
            &GraderSpec::FileExists {
                path: "nope.json".into()
            },
            &t,
            &dir
        )
        .pass
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- statistics reference values ---

#[test]
fn pass_at_k_reference_values() {
    assert_eq!(pass_at_k(5, 3, 1), Some(0.6));
    // 1 - C(2,2)/C(5,2) = 0.9 exactly.
    let v = pass_at_k(5, 3, 2).unwrap();
    assert!((v - 0.9).abs() < 1e-12, "{v}");
    // 1 - C(6,5)/C(10,5) = 1 - 6/252.
    let v = pass_at_k(10, 4, 5).unwrap();
    assert!((v - (1.0 - 6.0 / 252.0)).abs() < 1e-12, "{v}");
    assert_eq!(pass_at_k(4, 0, 2), Some(0.0));
    assert_eq!(pass_at_k(4, 4, 2), Some(1.0));
    assert_eq!(pass_at_k(0, 0, 1), None);
    assert_eq!(pass_at_k(3, 2, 5), None);
}

#[test]
fn wilson_reference_values() {
    // n=30, p=0.7, z=1.96 -> [0.5212, 0.8333] (standard Wilson table value).
    let (lo, hi) = wilson_interval(0.7, 30, 1.959963984540054);
    assert!((lo - 0.5212).abs() < 5e-4, "{lo}");
    assert!((hi - 0.8333).abs() < 5e-4, "{hi}");
    // Edges clamp.
    let (lo, hi) = wilson_interval(0.0, 10, 1.96);
    assert!(lo >= 0.0 && hi <= 1.0 && lo <= hi);
    assert_eq!(wilson_interval(0.5, 0, 1.96), (0.0, 1.0));
}

#[test]
fn bootstrap_is_deterministic_and_sane() {
    let base = vec![1.0; 10];
    let same = vec![1.0; 10];
    let (d, lo, hi) = bootstrap_paired_delta(&base, &same, 500).unwrap();
    assert_eq!((d, lo, hi), (0.0, 0.0, 0.0));
    // Same input twice -> identical output (seeded).
    let degraded: Vec<f64> = vec![1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0];
    let r1 = bootstrap_paired_delta(&base, &degraded, 500).unwrap();
    let r2 = bootstrap_paired_delta(&base, &degraded, 500).unwrap();
    assert_eq!(r1, r2);
    assert!((r1.0 - -0.2).abs() < 1e-12, "{r1:?}");
    assert!(r1.2 <= 0.0, "{r1:?}");
    // Total collapse is unambiguous.
    let collapsed = vec![0.0; 10];
    let (d, lo, hi) = bootstrap_paired_delta(&base, &collapsed, 200).unwrap();
    assert_eq!((d, lo, hi), (-1.0, -1.0, -1.0));
    assert_eq!(bootstrap_paired_delta(&[], &[], 100), None);
    assert_eq!(bootstrap_paired_delta(&[1.0], &[1.0, 0.0], 100), None);
}

// --- gate on synthetic datasets ---

fn record_with(cases: Vec<CaseRecord>) -> RunRecord {
    RunRecord {
        schema_version: 1,
        run_id: "test".into(),
        suite: "s".into(),
        suite_version: 1,
        code_version: "dirty".into(),
        ref_name: "work".into(),
        started_at_ms: 0,
        target: None,
        files_root: ".".into(),
        trials_total: cases.iter().map(|c| c.trials.len()).sum(),
        cases,
        gate: None,
    }
}

fn passing_case(id: &str, text: &str) -> CaseRecord {
    let trials = vec![trial_ok(id, text)];
    let grades = vec![Grade {
        grader: "regex[output.text]".into(),
        pass: true,
        score: Some(1.0),
        evidence: "match".into(),
    }];
    CaseRecord {
        case_id: id.into(),
        deterministic: true,
        trials: trials.clone(),
        grades,
        pass: true,
        digest: digest_trials(&trials),
    }
}

fn policy() -> GatePolicy {
    GatePolicy::default()
}

#[test]
fn gate_quiet_on_identical_runs() {
    let base = record_with(vec![passing_case("a", "x"), passing_case("b", "y")]);
    let cand = base.clone();
    let decision = decide_gate(Some(&base), &cand, &policy());
    assert!(decision.pass, "{decision:?}");
    assert!(decision.affected_cases.is_empty());
}

#[test]
fn gate_quiet_on_sampling_noise() {
    // Sampling cases with different (but passing) outputs are noise, not drift.
    let mut base = record_with(vec![passing_case("a", "x")]);
    base.cases[0].deterministic = false;
    let mut cand = record_with(vec![passing_case("a", "x-variant")]);
    cand.cases[0].deterministic = false;
    // Recompute: sampling case still passes its graders.
    let decision = decide_gate(Some(&base), &cand, &policy());
    assert!(decision.pass, "{decision:?}");
}

#[test]
fn gate_fires_on_deterministic_drift() {
    let base = record_with(vec![passing_case("a", "score=71")]);
    let cand = record_with(vec![passing_case("a", "score=12")]);
    let decision = decide_gate(Some(&base), &cand, &policy());
    assert!(!decision.pass, "{decision:?}");
    assert_eq!(decision.affected_cases, vec!["a".to_string()]);
    assert!(decision.evidence.iter().any(|e| e.contains("digest")));
}

#[test]
fn gate_fires_on_suite_level_bootstrap_degradation() {
    // 10 deterministic cases, baseline all green. Candidate collapses
    // on 6 of 10 with identical per-case evidence ("score dropped").
    // min_pass_rate is 0 so ONLY the paired bootstrap can fire.
    let mut policy = policy();
    policy.min_pass_rate = 0.0;
    policy.delta_floor = 0.1;
    let base_cases: Vec<CaseRecord> = (0..10)
        .map(|i| passing_case(&format!("c{i:02}"), "score=71"))
        .collect();
    let base = record_with(base_cases);
    let mut cand_cases = Vec::new();
    for i in 0..10 {
        let mut case = passing_case(&format!("c{i:02}"), "score=71");
        if i >= 4 {
            case.grades[0].pass = false;
            case.grades[0].evidence = "trial 0: score=12, expected score=71".into();
            case.pass = false;
        }
        cand_cases.push(case);
    }
    // Deterministic digests of passing cases are unchanged; failing
    // cases flip grades. Digests of failed cases differ too, but the
    // assertion below pins the bootstrap evidence specifically.
    let cand = record_with(cand_cases);
    let decision = decide_gate(Some(&base), &cand, &policy);
    assert!(!decision.pass, "{decision:?}");
    assert!(
        decision.evidence.iter().any(|e| e.contains("paired delta")),
        "{decision:?}"
    );
    assert!(
        decision
            .uncertainty
            .as_deref()
            .unwrap()
            .contains("bootstrap"),
        "{decision:?}"
    );
}

#[test]
fn gate_fires_on_grade_flip_and_names_evidence() {
    let base = record_with(vec![passing_case("a", "x")]);
    let mut cand = base.clone();
    cand.cases[0].grades[0].pass = false;
    cand.cases[0].grades[0].evidence = "trial 0: exit_code=1, expected 0".into();
    cand.cases[0].pass = false;
    let decision = decide_gate(Some(&base), &cand, &policy());
    assert!(!decision.pass);
    assert!(decision.evidence.iter().any(|e| e.contains("exit_code=1")));
}

#[test]
fn gate_without_baseline_records_but_passes() {
    let cand = record_with(vec![passing_case("a", "x")]);
    let decision = decide_gate(None, &cand, &policy());
    assert!(decision.pass);
    assert!(decision.what_changed.contains("no baseline"));
}
