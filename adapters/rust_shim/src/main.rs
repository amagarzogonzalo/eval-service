//! Rust reference target speaking `eval/v1` (worker + oneshot).
//!
//! Behavior is driven per case by `input.params.behavior`:
//! echo (default), scrub, error, exit, hang, crash, malformed,
//! cv_write, digest_ok, digest_weak, digest_sample.
//!
//! Exit codes: 0 normal, 2 handshake/schema violation, 3 crash behavior.

use eval_protocol as proto;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut mode = proto::Mode::Worker;
    let mut handshook = false;
    for line in lines.by_ref() {
        let line = match line {
            Ok(line) => line,
            Err(_) => std::process::exit(2),
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => fail("handshake is not JSON"),
        };
        if !handshook {
            let handshake: proto::HandshakeStrict = match serde_json::from_value(value) {
                Ok(handshake) => handshake,
                Err(_) => fail("handshake schema violation"),
            };
            if handshake.protocol != proto::PROTOCOL_VERSION {
                fail(&format!("version mismatch: {}", handshake.protocol));
            }
            mode = handshake.mode;
            emit(&proto::encode_ready());
            handshook = true;
            if mode == proto::Mode::Oneshot {
                continue;
            }
            continue;
        }
        if value.get("type").and_then(|t| t.as_str()) == Some("shutdown") {
            return;
        }
        let case: proto::CaseRequestStrict = match serde_json::from_value(value) {
            Ok(case) => case,
            Err(_) => fail("case schema violation (strict target)"),
        };
        handle_case(&case);
        if mode == proto::Mode::Oneshot {
            return;
        }
    }
}

fn fail(message: &str) -> ! {
    eprintln!("eval-shim-rs: {message}");
    std::process::exit(2);
}

fn emit(line: &str) {
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn params(case: &proto::CaseRequestStrict) -> serde_json::Map<String, Value> {
    // InputStrict.params is Option<Value>; normalize to an object map.
    match &case.input.params {
        Some(Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    }
}

fn param_string(params: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn handle_case(case: &proto::CaseRequestStrict) {
    let params = params(case);
    let behavior = param_string(&params, "behavior").unwrap_or_else(|| "echo".to_string());
    match behavior.as_str() {
        "hang" => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        "crash" => std::process::exit(3),
        "exit" => {
            let code = params.get("code").and_then(Value::as_i64).unwrap_or(1) as i32;
            std::process::exit(code);
        }
        "malformed" => {
            println!("this is not json at all");
            let _ = std::io::stdout().flush();
        }
        "error" => {
            let code = param_string(&params, "code").unwrap_or_else(|| "failed".to_string());
            let message =
                param_string(&params, "message").unwrap_or_else(|| "handled failure".to_string());
            let retriable = params
                .get("retriable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            emit(&proto::encode_case_error(&proto::CaseErrorMsg {
                kind: proto::CaseErrorKind::CaseError,
                case_id: case.case_id.clone(),
                code,
                message,
                retriable,
            }));
        }
        _ => {
            let sampled_weak = behavior == "digest_sample" && nanos_mod(5) == 4;
            let (text, files) = demo_payload(case, &params, &behavior, sampled_weak);
            emit_event(case, 0, "stage_done", json!({"stage": "extract"}));
            let extra_events = params.get("reviews").and_then(Value::as_u64).unwrap_or(0);
            for i in 0..extra_events {
                emit_event(
                    case,
                    i + 1,
                    &format!("review_{i}"),
                    json!({"review": i, "budget": 6}),
                );
            }
            if behavior == "digest_ok" || behavior == "digest_weak" || behavior == "digest_sample" {
                let verdict = if behavior == "digest_weak" || sampled_weak {
                    "weak"
                } else {
                    "ok"
                };
                emit_event(
                    case,
                    extra_events + 1,
                    "verdict",
                    json!({"verdict": verdict}),
                );
            }
            emit(&proto::encode_result(&proto::ResultMsg {
                kind: proto::ResultKind::Result,
                case_id: case.case_id.clone(),
                ok: true,
                output: Some(proto::Output {
                    text,
                    files,
                    data: None,
                }),
                metrics: Some(proto::Metrics { wall_ms: 1 }),
                ai: None,
            }));
        }
    }
}

fn emit_event(case: &proto::CaseRequestStrict, seq: u64, name: &str, data: Value) {
    emit(&proto::encode_event(&proto::Event {
        kind: proto::EventKind::Event,
        case_id: case.case_id.clone(),
        seq,
        name: name.to_string(),
        data: Some(data),
    }));
}

/// Payload shared by echo-like behaviors (mirrored in the Python shim).
fn demo_payload(
    case: &proto::CaseRequestStrict,
    params: &serde_json::Map<String, Value>,
    behavior: &str,
    sampled_weak: bool,
) -> (String, Vec<String>) {
    let stdin_text = case.input.stdin_text.clone();
    match behavior {
        "scrub" => (scrub_canary(&stdin_text), Vec::new()),
        "cv_write" => {
            let path =
                param_string(params, "write_path").unwrap_or_else(|| "output.txt".to_string());
            let content = param_string(params, "write_content").unwrap_or_else(|| "{}".to_string());
            if let Some(parent) = std::path::Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            let _ = std::fs::write(&path, &content);
            let text = param_string(params, "text").unwrap_or_else(|| "score=71.2".to_string());
            (text, vec![path])
        }
        "digest_ok" => (good_summary(params), Vec::new()),
        "digest_weak" => (weak_summary(), Vec::new()),
        "digest_sample" => {
            if sampled_weak {
                (weak_summary(), Vec::new())
            } else {
                (good_summary(params), Vec::new())
            }
        }
        _ => {
            if let Some(text) = param_string(params, "text") {
                (text, Vec::new())
            } else {
                (stdin_text, Vec::new())
            }
        }
    }
}

fn nanos_mod(m: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % m)
        .unwrap_or(0)
}

fn good_summary(params: &serde_json::Map<String, Value>) -> String {
    let n = params.get("reviews").and_then(Value::as_u64).unwrap_or(3);
    format!(
        "Digest with {n} reviews processed within budget. Findings: latency p99 down 12% [1], error budget intact [2]. {}",
        "detail ".repeat(40)
    )
}

fn weak_summary() -> String {
    "pending".to_string()
}

fn scrub_canary(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("CANARY-") {
        out.push_str(&rest[..start]);
        out.push_str("CANARY-<SCRUBBED>");
        let tail = &rest[start + "CANARY-".len()..];
        let token_len = tail
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(tail.len());
        rest = &tail[token_len..];
    }
    out.push_str(rest);
    out
}
