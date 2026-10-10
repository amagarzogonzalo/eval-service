use eval_core::TrialObservation;
use serde_json::Value;

/// Extract a field from a trial observation.
///
/// Paths: `status`, `exit_code`, `wall_ms`, `stderr`, `error_code`,
/// `error_message`, `output.<path…>`, `metrics.<path…>` (shorthand for
/// `output.metrics.<path…>`), `ai.<path…>` (shorthand for
/// `output.ai.<path…>`), `events[<i>].<path…>`, `events.len`.
pub fn extract_field(trial: &TrialObservation, field: &str) -> Option<Value> {
    if field == "status" {
        return Some(Value::String(trial.status.as_str().to_string()));
    }
    if field == "exit_code" {
        return Some(trial.exit_code.map_or(Value::Null, |c| c.into()));
    }
    if field == "wall_ms" {
        return Some(trial.wall_ms.into());
    }
    if field == "stderr" {
        return Some(trial.stderr_tail.clone().map_or(Value::Null, Value::String));
    }
    if field == "error_code" {
        return Some(trial.error_code.clone().map_or(Value::Null, Value::String));
    }
    if field == "error_message" {
        return Some(
            trial
                .error_message
                .clone()
                .map_or(Value::Null, Value::String),
        );
    }
    if let Some(rest) = field.strip_prefix("output.") {
        let direct = trial.output.as_ref().and_then(|o| walk(o, rest));
        if direct.is_some() {
            return direct;
        }
        // Ergonomic fallback: `output.text` also resolves
        // `output.output.text` (the result message nests the target
        // payload one level deeper).
        return trial
            .output
            .as_ref()
            .and_then(|o| o.get("output"))
            .and_then(|o| walk(o, rest));
    }
    if let Some(rest) = field.strip_prefix("metrics.") {
        return trial
            .output
            .as_ref()
            .and_then(|o| o.get("metrics"))
            .and_then(|m| walk(m, rest));
    }
    if let Some(rest) = field.strip_prefix("ai.") {
        return trial
            .output
            .as_ref()
            .and_then(|o| o.get("ai"))
            .and_then(|m| walk(m, rest));
    }
    if field == "events.len" {
        return Some((trial.events.len() as u64).into());
    }
    if let Some(rest) = field.strip_prefix("events[")
        && let Some(end) = rest.find(']')
    {
        let idx: usize = rest[..end].parse().ok()?;
        let after = rest[end + 1..].strip_prefix('.').unwrap_or("");
        let event = trial.events.get(idx)?;
        if after.is_empty() {
            return Some(event.clone());
        }
        return walk(event, after);
    }
    None
}

fn walk(value: &Value, path: &str) -> Option<Value> {
    if path.is_empty() {
        return Some(value.clone());
    }
    let (head, tail) = match path.find('.') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (path, ""),
    };
    // Array index step: `files[0]`.
    if let Some(bracket) = head.find('[')
        && head.ends_with(']')
    {
        let key = &head[..bracket];
        let idx: usize = head[bracket + 1..head.len() - 1].parse().ok()?;
        let arr = value.get(key)?.as_array()?;
        return walk(arr.get(idx)?, tail);
    }
    walk(value.get(head)?, tail)
}

/// Render a value for evidence lines: strings verbatim, else canonical JSON.
pub fn value_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "<absent>".to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<unprintable>".to_string()),
    }
}
