//! M1 acceptance tests for `eval-protocol`.
//!
//! - Schema drift check (committed `protocol/eval-protocol-v1.schema.json`)
//! - Version-mismatch rejection
//! - Unknown-field tolerance (runner) vs strictness (target)
//! - Ordering / correlation (gapless seq, out-of-order rejected)
//! - Line-size cap, malformed lines, timeout-rule helper

use eval_protocol::*;
use std::collections::HashMap;

fn sample_case() -> CaseRequest {
    CaseRequest {
        kind: CaseKind::Case,
        case_id: "cv-eval-03".to_string(),
        input: Input {
            argv: vec!["eval".to_string(), "output/build/cv_ink.pdf".to_string()],
            stdin_text: String::new(),
            params: None,
        },
        limits: Limits {
            timeout_ms: 30_000,
            max_output_bytes: 1_048_576,
        },
        env: HashMap::from([("OFFLINE".to_string(), "1".to_string())]),
    }
}

#[test]
fn handshake_round_trip_both_modes() {
    for mode in [Mode::Worker, Mode::Oneshot] {
        let line = encode_handshake(mode);
        let hs = parse_handshake(&line).expect("handshake parses");
        assert_eq!(negotiate(&hs).expect("negotiates"), mode);
    }
}

#[test]
fn version_mismatch_rejected() {
    // Handshake with wrong major version.
    let line = r#"{"protocol":"eval/v9","mode":"worker"}"#;
    let hs = parse_handshake(line).expect("shape parses");
    let err = negotiate(&hs).expect_err("must reject");
    assert_eq!(
        err,
        ProtocolError::VersionMismatch {
            expected: "eval/v1".to_string(),
            received: "eval/v9".to_string(),
        }
    );

    // Ready with wrong version rejected at parse_line.
    let ready = r#"{"protocol":"eval/v2","ready":true}"#;
    let err = parse_line(ready).expect_err("ready version must reject");
    assert!(matches!(err, ProtocolError::VersionMismatch { .. }));
}

#[test]
fn runner_tolerates_unknown_fields_target_rejects() {
    // Runner side: unknown fields ignored (forward compatible).
    let line = r#"{"protocol":"eval/v1","mode":"worker","future_field":123}"#;
    let hs = parse_handshake(line).expect("tolerant parse keeps working");
    assert_eq!(hs.mode, Mode::Worker);

    let case_line = r#"{"type":"case","case_id":"x","input":{"argv":[],"stdin_text":""},"limits":{"timeout_ms":1000,"max_output_bytes":1024},"env":{},"future":true}"#;
    let case = parse_case(case_line).expect("tolerant case parse works");
    assert_eq!(case.case_id, "x");

    // Target side: strict parse rejects the same lines.
    assert!(parse_handshake_strict(line).is_err());
    let strict_case = r#"{"type":"case","case_id":"x","input":{"argv":[]},"limits":{"timeout_ms":1000,"max_output_bytes":1024},"bogus":1}"#;
    assert!(parse_case_strict(strict_case).is_err());
}

#[test]
fn parse_line_happy_path_event_result_case_error() {
    let ev = parse_line(
        r#"{"type":"event","case_id":"a","seq":0,"name":"stage_done","data":{"stage":"extract"}}"#,
    );
    assert!(matches!(ev, Ok(Inbound::Event(_))));

    // Unknown fields on inbound tolerated.
    let ev2 = parse_line(r#"{"type":"event","case_id":"a","seq":1,"name":"x","future":1}"#);
    assert!(matches!(ev2, Ok(Inbound::Event(_))));

    let res = parse_line(
        r#"{"type":"result","case_id":"a","ok":true,"output":{"text":"score=71.2","files":[]},"metrics":{"wall_ms":4120}}"#,
    );
    assert!(matches!(res, Ok(Inbound::Result(_))));

    let ce = parse_line(
        r#"{"type":"case_error","case_id":"a","code":"no_sources","message":"empty","retriable":false}"#,
    );
    assert!(matches!(ce, Ok(Inbound::CaseError(_))));

    let ready = parse_line(r#"{"protocol":"eval/v1","ready":true}"#);
    assert!(matches!(ready, Ok(Inbound::Ready(_))));
}

#[test]
fn parse_line_rejects_malformed_and_unknown_type() {
    assert!(matches!(
        parse_line("not json"),
        Err(ProtocolError::Json(_))
    ));
    assert!(matches!(
        parse_line(r#"{"type":"nope","case_id":"a"}"#),
        Err(ProtocolError::UnknownMessageType(_))
    ));
    assert!(matches!(
        parse_line(r#"{"no_type":1}"#),
        Err(ProtocolError::SchemaViolation(_))
    ));
    // stdout garbage is a protocol violation, never silently skipped.
    assert!(parse_line("hello world").is_err());
}

#[test]
fn seq_tracker_enforces_gapless_order() {
    let mut t = SeqTracker::new();
    let ev = |seq| Event {
        kind: EventKind::Event,
        case_id: "a".to_string(),
        seq,
        name: "s".to_string(),
        data: None,
    };
    t.push_event(&ev(0)).unwrap();
    t.push_event(&ev(1)).unwrap();
    // Gap rejected.
    assert!(t.push_event(&ev(3)).is_err());
    // Repeat of old seq rejected.
    assert!(t.push_event(&ev(1)).is_err());
    t.finish_case("a");
}

#[test]
fn validate_case_catches_empty_id_and_zero_limits() {
    let mut c = sample_case();
    assert!(validate_case(&c).is_empty());
    c.case_id = "  ".to_string();
    assert!(!validate_case(&c).is_empty());
    c.case_id = "ok".to_string();
    c.limits.timeout_ms = 0;
    assert!(
        validate_case(&c)
            .iter()
            .any(|e| e.field.contains("timeout"))
    );
}

#[test]
fn timeout_rule_distinguishes_graceful_from_kill() {
    assert!(satisfies_timeout_rule(30_000, 20_000)); // exactly 1.5x
    assert!(!satisfies_timeout_rule(29_999, 20_000));
    assert!(satisfies_timeout_rule(1_000, 0)); // no declared budget
}

#[test]
fn line_size_cap_enforced() {
    let big = "x".repeat(MAX_LINE_BYTES + 1);
    let line = format!(r#"{{"type":"event","case_id":"a","seq":0,"name":"{big}"}}"#);
    // Construct directly to avoid huge format cost confusion: check size gate first.
    assert!(line.len() > MAX_LINE_BYTES);
    assert!(matches!(
        parse_line(&line),
        Err(ProtocolError::LineTooLong { .. })
    ));
}

#[test]
fn shutdown_and_ready_encode() {
    assert_eq!(encode_shutdown(), r#"{"type":"shutdown"}"#);
    let ready = encode_ready();
    assert!(matches!(parse_line(&ready), Ok(Inbound::Ready(_))));
}

#[test]
fn schema_drift_check() {
    // Compares generated schema against the committed file.
    // Run with UPDATE_SCHEMA=1 to regenerate (human must review the diff).
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let schema_path = manifest.join("../../protocol/eval-protocol-v1.schema.json");
    let generated = schema_json_pretty();
    if std::env::var("UPDATE_SCHEMA").as_deref() == Ok("1") {
        std::fs::create_dir_all(schema_path.parent().unwrap()).unwrap();
        std::fs::write(&schema_path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|_| panic!("missing {}", schema_path.display()));
    assert_eq!(
        committed, generated,
        "schema drift: run with UPDATE_SCHEMA=1 to regenerate and review the diff"
    );
}
