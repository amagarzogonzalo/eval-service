use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// Replacement marker for secret material. Never empty, always greppable.
pub const REDACTED: &str = "<REDACTED>";

fn patterns() -> &'static Vec<(Regex, &'static str)> {
    static CELL: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    CELL.get_or_init(|| {
        vec![
            (
                Regex::new(r"(?i)\b(authorization)\s*:\s*(bearer\s+)?[A-Za-z0-9\-._~+/=]+")
                    .expect("auth pattern compiles"),
                "$1: <REDACTED>",
            ),
            (
                Regex::new(
                    r#"(?i)\b(api[_-]?key|x-api-key|token|secret|password|passwd|pwd)\b\s*[:=]\s*("[^"]*"|'[^']*'|\S+)"#,
                )
                .expect("key=value pattern compiles"),
                "$1=<REDACTED>",
            ),
            (
                Regex::new(r"(?i)\b(bearer)\s+[A-Za-z0-9\-._~+/=]+")
                    .expect("bearer pattern compiles"),
                "Bearer <REDACTED>",
            ),
            (
                Regex::new(r"\bsk-[A-Za-z0-9]{16,}\b").expect("sk pattern compiles"),
                "sk-<REDACTED>",
            ),
        ]
    })
}

/// Redact secret shapes in free text. Idempotent.
pub fn redact_string(s: &str) -> String {
    let mut out = s.to_string();
    for (re, replacement) in patterns() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    out
}

/// Recursively redact every string inside a JSON value.
pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(redact_string(s)),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (redact_string(k), redact_json(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}
