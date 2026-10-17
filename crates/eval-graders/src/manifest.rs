//! Machine-readable self-description of the platform.
//!
//! `eval schema` emits this so an agent can learn the whole integration
//! surface in one call instead of reading prose or parsing `--help` N
//! times (see docs/integration.md). Everything here is derived from the
//! real enums/definitions the engine uses, so it cannot silently drift:
//! [`grader_kinds`] is an exhaustive match over `GraderSpec`, and
//! [`field_paths`] mirrors the extractor. A test asserts the manifest
//! covers every grader the engine accepts.

use serde::{Deserialize, Serialize};

/// Canonical manifest version. Bump on any breaking shape change.
pub const MANIFEST_VERSION: u32 = 1;

/// Exit-code contract, machine-readable so an agent can classify a
/// result from the exit code alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitCodeDoc {
    pub code: i32,
    pub name: String,
    pub meaning: String,
    /// Safe to re-run with the same inputs (no partial state).
    pub retryable: bool,
    /// `none` | `partial` | `complete` — what the run may have written.
    pub side_effects: String,
}

/// One grader kind and what it asserts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraderDoc {
    pub kind: String,
    pub asserts: String,
    pub example: serde_json::Value,
}

/// Full platform manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSchema {
    pub manifest_version: u32,
    pub platform: String,
    pub protocol: String,
    pub exit_codes: Vec<ExitCodeDoc>,
    pub grader_kinds: Vec<GraderDoc>,
    pub field_paths: Vec<String>,
    pub suite_keys: Vec<String>,
    pub protocol_messages: Vec<String>,
    pub notes: Vec<String>,
}

/// Every grader kind the engine accepts, as the YAML `kind` value.
/// Exhaustive over `GraderSpec`: adding a variant without updating this
/// list is a compile error, which keeps the manifest honest.
pub fn grader_kinds() -> Vec<&'static str> {
    let variants = [
        eval_core::GraderSpec::ExitCode { expect: 0 },
        eval_core::GraderSpec::Exact {
            field: String::new(),
            value: String::new(),
        },
        eval_core::GraderSpec::Regex {
            field: String::new(),
            pattern: String::new(),
        },
        eval_core::GraderSpec::NotRegex {
            field: String::new(),
            pattern: String::new(),
        },
        eval_core::GraderSpec::Range {
            field: String::new(),
            min: None,
            max: None,
        },
        eval_core::GraderSpec::JsonSchema {
            field: None,
            schema: serde_json::json!({}),
        },
        eval_core::GraderSpec::Stability,
        eval_core::GraderSpec::FileExists {
            path: String::new(),
        },
        eval_core::GraderSpec::FileMatches {
            path: String::new(),
            pattern: String::new(),
        },
        eval_core::GraderSpec::JudgeLite {
            field: String::new(),
            min_score: 0.0,
        },
    ];
    // Exhaustiveness guard: this match fails to compile if a new variant
    // is added to GraderSpec without a line here.
    for spec in &variants {
        match spec {
            eval_core::GraderSpec::ExitCode { .. }
            | eval_core::GraderSpec::Exact { .. }
            | eval_core::GraderSpec::Regex { .. }
            | eval_core::GraderSpec::NotRegex { .. }
            | eval_core::GraderSpec::Range { .. }
            | eval_core::GraderSpec::JsonSchema { .. }
            | eval_core::GraderSpec::Stability
            | eval_core::GraderSpec::FileExists { .. }
            | eval_core::GraderSpec::FileMatches { .. }
            | eval_core::GraderSpec::JudgeLite { .. } => {}
        }
    }
    let mut kinds: Vec<&'static str> = variants.iter().map(grader_kind_name).collect();
    kinds.sort_unstable();
    kinds
}

fn grader_kind_name(spec: &eval_core::GraderSpec) -> &'static str {
    match spec {
        eval_core::GraderSpec::ExitCode { .. } => "exit_code",
        eval_core::GraderSpec::Exact { .. } => "exact",
        eval_core::GraderSpec::Regex { .. } => "regex",
        eval_core::GraderSpec::NotRegex { .. } => "not_regex",
        eval_core::GraderSpec::Range { .. } => "range",
        eval_core::GraderSpec::JsonSchema { .. } => "json_schema",
        eval_core::GraderSpec::Stability => "stability",
        eval_core::GraderSpec::FileExists { .. } => "file_exists",
        eval_core::GraderSpec::FileMatches { .. } => "file_matches",
        eval_core::GraderSpec::JudgeLite { .. } => "judge_lite",
    }
}

/// Field paths the graders can read. Mirrors `eval_graders::extract_field`;
/// a test keeps the two in step.
pub fn field_paths() -> Vec<&'static str> {
    vec![
        "status",
        "exit_code",
        "wall_ms",
        "stderr",
        "error_code",
        "error_message",
        "output.<path>",
        "metrics.<path>",
        "ai.<path>",
        "events.len",
        "events[<i>].<path>",
    ]
}

/// Build the manifest.
pub fn platform_schema() -> PlatformSchema {
    let grader_docs = vec![
        GraderDoc {
            kind: "exit_code".into(),
            asserts: "process exit code equals `expect`".into(),
            example: serde_json::json!({"kind": "exit_code", "expect": 0}),
        },
        GraderDoc {
            kind: "exact".into(),
            asserts: "field string equals `value`".into(),
            example: serde_json::json!({"kind": "exact", "field": "output.text", "value": "score=71.2"}),
        },
        GraderDoc {
            kind: "regex".into(),
            asserts: "field matches `pattern` (Rust regex)".into(),
            example: serde_json::json!({"kind": "regex", "field": "output.text", "pattern": "PR #\\d+"}),
        },
        GraderDoc {
            kind: "not_regex".into(),
            asserts: "field does NOT match `pattern` (canary/secret absence)".into(),
            example: serde_json::json!({"kind": "not_regex", "field": "output.text", "pattern": "CANARY-\\w+"}),
        },
        GraderDoc {
            kind: "range".into(),
            asserts: "numeric field within [min, max] (either bound optional)".into(),
            example: serde_json::json!({"kind": "range", "field": "metrics.wall_ms", "min": 0, "max": 9000}),
        },
        GraderDoc {
            kind: "json_schema".into(),
            asserts: "field (default the result envelope) validates a JSON-Schema subset: type/required/properties/enum/const".into(),
            example: serde_json::json!({"kind": "json_schema", "field": "output.data",
                "schema": {"type": "object", "required": ["name", "email"]}}),
        },
        GraderDoc {
            kind: "stability".into(),
            asserts: "case-level: normalized output identical across >= 2 trials (infra failures never count as stable)".into(),
            example: serde_json::json!({"kind": "stability"}),
        },
        GraderDoc {
            kind: "file_exists".into(),
            asserts: "artifact path exists (relative to the run cwd)".into(),
            example: serde_json::json!({"kind": "file_exists", "path": "output/reports/cv_summary.json"}),
        },
        GraderDoc {
            kind: "file_matches".into(),
            asserts: "artifact file content matches `pattern` (<= 1 MiB)".into(),
            example: serde_json::json!({"kind": "file_matches", "path": "output/report.json", "pattern": "\"score\": 71"}),
        },
        GraderDoc {
            kind: "judge_lite".into(),
            asserts: "deterministic text-quality score >= `min_score` (stand-in judge; live judges are opt-in)".into(),
            example: serde_json::json!({"kind": "judge_lite", "field": "output.text", "min_score": 0.5}),
        },
    ];
    PlatformSchema {
        manifest_version: MANIFEST_VERSION,
        platform: "eval-service".into(),
        protocol: "eval/v1".into(),
        exit_codes: vec![
            ExitCodeDoc {
                code: 0,
                name: "pass".into(),
                meaning: "all cases passed".into(),
                retryable: true,
                side_effects: "complete".into(),
            },
            ExitCodeDoc {
                code: 1,
                name: "fail".into(),
                meaning: "a case failed, regressed, or observed a timeout".into(),
                retryable: true,
                side_effects: "complete".into(),
            },
            ExitCodeDoc {
                code: 2,
                name: "infra".into(),
                meaning: "suite/target/store error; the run could not be evaluated (includes crash/protocol_error)".into(),
                retryable: false,
                side_effects: "partial".into(),
            },
        ],
        grader_kinds: grader_docs,
        field_paths: field_paths().into_iter().map(str::to_string).collect(),
        suite_keys: vec![
            "suite".into(),
            "version".into(),
            "target.argv".into(),
            "target.mode".into(),
            "target.budget_ms".into(),
            "target.cwd".into(),
            "env_allowlist".into(),
            "gate.min_pass_rate".into(),
            "gate.delta_floor".into(),
            "cases[].id".into(),
            "cases[].tags".into(),
            "cases[].input.argv".into(),
            "cases[].input.stdin_text".into(),
            "cases[].input.params".into(),
            "cases[].limits.timeout_ms".into(),
            "cases[].limits.max_output_bytes".into(),
            "cases[].trials".into(),
            "cases[].deterministic".into(),
            "cases[].graders[]".into(),
        ],
        protocol_messages: vec![
            "runner->target: {\"protocol\":\"eval/v1\",\"mode\":\"worker|oneshot\"}".into(),
            "target->runner: {\"protocol\":\"eval/v1\",\"ready\":true}".into(),
            "runner->target: {\"type\":\"case\",\"case_id\":..,\"input\":{\"argv\":[],\"stdin_text\":\"\",\"params\":{}},\"limits\":{\"timeout_ms\":..,\"max_output_bytes\":..},\"env\":{}}".into(),
            "target->runner: {\"type\":\"event\",\"case_id\":..,\"seq\":0,\"name\":..,\"data\":{}}".into(),
            "target->runner: {\"type\":\"result\",\"case_id\":..,\"ok\":true,\"output\":{\"text\":..,\"files\":[],\"data\":{}},\"metrics\":{\"wall_ms\":..},\"ai\":{\"model\":..,\"tokens_in\":..,\"tokens_out\":..}}".into(),
            "target->runner: {\"type\":\"case_error\",\"case_id\":..,\"code\":..,\"message\":..,\"retriable\":false}".into(),
            "runner->target: {\"type\":\"shutdown\"}".into(),
        ],
        notes: vec![
            "The target is YOUR program: it is spawned as a subprocess and speaks eval/v1 JSONL on stdin/stdout. stderr is free-form logs, never parsed.".into(),
            "Run `eval check <suite.yaml> --probe` to validate a suite and conformance-test the adapter deterministically before grading.".into(),
            "Usage/cost: emit llm/v1 events (see docs/llm-events.md); import with `eval usage-import`. Absent metrics stay n/a, never zero.".into(),
            "Graders test properties, never exact wording, for model output. Use trials>=2 + stability or judge_lite for nondeterministic targets.".into(),
        ],
    }
}

/// Pretty JSON manifest for the `schema` command.
pub fn schema_json_pretty() -> String {
    let mut json = serde_json::to_string_pretty(&platform_schema()).expect("manifest serializes");
    json.push('\n');
    json
}
