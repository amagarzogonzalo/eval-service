//! Tests for the machine-readable manifest and the deterministic
//! integration validator (`eval schema` / `eval check`).

use eval_core::*;
use eval_graders::manifest::{MANIFEST_VERSION, grader_kinds, platform_schema};
use eval_graders::validate::{Severity, check_suite};

fn suite_with(graders: Vec<GraderSpec>, trials: u32, deterministic: bool) -> Suite {
    Suite {
        suite: "t".into(),
        version: 1,
        target: Some(TargetSpec {
            argv: vec!["$PYTHON".into(), "adapter.py".into()],
            mode: TargetMode::Oneshot,
            budget_ms: None,
            cwd: None,
        }),
        env_allowlist: Vec::new(),
        gate: Default::default(),
        cases: vec![CaseSpec {
            id: "a".into(),
            tags: Vec::new(),
            input: InputSpec::default(),
            limits: LimitsSpec {
                timeout_ms: 1000,
                max_output_bytes: 1024,
            },
            trials,
            deterministic,
            presets: Vec::new(),
            graders,
        }],
    }
}

#[test]
fn manifest_covers_every_grader_kind() {
    // The manifest is the agent-facing contract; it must name exactly the
    // graders the engine accepts (grader_kinds is an exhaustive match).
    let schema = platform_schema();
    assert_eq!(schema.manifest_version, MANIFEST_VERSION);
    let mut documented: Vec<&str> = schema
        .grader_kinds
        .iter()
        .map(|g| g.kind.as_str())
        .collect();
    documented.sort_unstable();
    let actual = grader_kinds();
    assert_eq!(documented, actual);
    assert_eq!(documented.len(), 10, "one entry per GraderSpec variant");
    // Every documented grader carries a usable example.
    for doc in &schema.grader_kinds {
        assert!(
            doc.example.get("kind").is_some(),
            "grader {} example lacks `kind`",
            doc.kind
        );
    }
}

#[test]
fn manifest_advertises_run_metrics_contract() {
    let schema = platform_schema();
    assert_eq!(
        schema.run_manifest_version,
        eval_core::run_manifest::RUN_MANIFEST_VERSION
    );
    assert_eq!(
        schema.metrics_version,
        eval_core::run_manifest::METRICS_SCHEMA_VERSION
    );
    assert_eq!(
        schema.run_lanes,
        vec!["batch", "local-service", "remote-service", "replay"]
    );
    for class in [
        "grader",
        "case_error",
        "timeout",
        "crash",
        "protocol_error",
        "service_unavailable",
        "budget_exceeded",
        "missing_artifact",
        "cancelled",
        "unknown",
    ] {
        assert!(
            schema.failure_classes.iter().any(|c| c == class),
            "missing failure class {class}"
        );
    }
    assert!(schema.run_manifest_keys.contains(&"lane".to_string()));
    assert!(
        schema
            .run_manifest_keys
            .contains(&"metrics.usage.tokens_in".to_string())
    );
    assert_eq!(
        schema.run_manifest_schema["properties"]["manifest_version"]["const"],
        serde_json::json!(eval_core::run_manifest::RUN_MANIFEST_VERSION)
    );
    assert!(
        schema.run_manifest_schema["properties"]["metrics"]["properties"]["outcomes"]["properties"]
            .get("timeouts")
            .is_some()
    );
}

#[test]
fn manifest_declares_exit_codes_and_protocol() {
    let schema = platform_schema();
    let codes: Vec<i32> = schema.exit_codes.iter().map(|c| c.code).collect();
    assert_eq!(codes, vec![0, 1, 2]);
    assert!(
        schema
            .exit_codes
            .iter()
            .any(|c| c.name == "infra" && !c.retryable)
    );
    assert_eq!(schema.protocol, "eval/v1");
    // The seven protocol message shapes are all present.
    assert_eq!(schema.protocol_messages.len(), 7);
    assert!(
        schema
            .protocol_messages
            .iter()
            .any(|m| m.contains("\"ready\""))
    );
    assert!(
        schema
            .protocol_messages
            .iter()
            .any(|m| m.contains("shutdown"))
    );
}

#[test]
fn check_accepts_a_valid_suite() {
    let suite = suite_with(
        vec![GraderSpec::ExitCode { expect: 0 }, GraderSpec::Stability],
        2,
        true,
    );
    let report = check_suite(&suite);
    assert!(report.ok(), "{:?}", report.issues);
    assert_eq!(report.errors(), 0);
}

#[test]
fn check_flags_bad_regex_with_a_hint() {
    let suite = suite_with(
        vec![GraderSpec::Regex {
            field: "output.text".into(),
            pattern: "([invalid".into(),
        }],
        1,
        true,
    );
    let report = check_suite(&suite);
    assert!(!report.ok());
    let issue = report
        .issues
        .iter()
        .find(|i| i.code == "grader.bad_regex")
        .expect("bad_regex issue");
    assert_eq!(issue.severity, Severity::Error);
    assert!(!issue.hint.is_empty());
}

#[test]
fn check_flags_stability_without_trials() {
    let suite = suite_with(vec![GraderSpec::Stability], 1, true);
    let report = check_suite(&suite);
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "grader.stability_needs_trials"),
        "{:?}",
        report.issues
    );
}

#[test]
fn check_allows_extra_trials_only_with_stability() {
    // Deterministic + 2 trials + stability is legitimate (how stability
    // is verified) and must NOT warn.
    let ok_stability = check_suite(&suite_with(vec![GraderSpec::Stability], 2, true));
    assert!(
        !ok_stability
            .issues
            .iter()
            .any(|i| i.code == "case.deterministic_multi_trial"),
        "{:?}",
        ok_stability.issues
    );
    // Deterministic + 2 trials without stability is a smell.
    let smell = check_suite(&suite_with(
        vec![GraderSpec::ExitCode { expect: 0 }],
        2,
        true,
    ));
    assert!(
        smell
            .issues
            .iter()
            .any(|i| i.code == "case.deterministic_multi_trial"),
        "{:?}",
        smell.issues
    );
}

#[test]
fn check_flags_inverted_range_and_bad_judge_score() {
    let inverted = check_suite(&suite_with(
        vec![GraderSpec::Range {
            field: "metrics.wall_ms".into(),
            min: Some(10.0),
            max: Some(1.0),
        }],
        1,
        true,
    ));
    assert!(
        inverted
            .issues
            .iter()
            .any(|i| i.code == "grader.range_inverted")
    );

    let judge = check_suite(&suite_with(
        vec![GraderSpec::JudgeLite {
            field: "output.text".into(),
            min_score: 2.0,
        }],
        1,
        true,
    ));
    assert!(
        judge
            .issues
            .iter()
            .any(|i| i.code == "grader.judge_score_range")
    );
}

#[test]
fn check_warns_on_unknown_field_root_and_no_graders() {
    let unknown = check_suite(&suite_with(
        vec![GraderSpec::Exact {
            field: "bogus.path".into(),
            value: "x".into(),
        }],
        1,
        true,
    ));
    assert!(
        unknown
            .issues
            .iter()
            .any(|i| i.code == "grader.unknown_field_root"),
        "{:?}",
        unknown.issues
    );

    let none = check_suite(&suite_with(vec![], 1, true));
    assert!(none.issues.iter().any(|i| i.code == "case.no_graders"));
}
