use crate::fields::{extract_field, value_string};
use eval_core::{CaseSpec, Grade, GraderSpec, TrialObservation, TrialStatus};
use serde_json::Value;
use std::path::Path;

/// Grade one trial against one grader spec. (`Stability` is case-level;
/// here it records a pass with a pointer to the case-level grade.)
pub fn grade_one(spec: &GraderSpec, trial: &TrialObservation, files_root: &Path) -> Grade {
    let name = spec.name();
    let fail = |evidence: String| Grade {
        grader: name.clone(),
        pass: false,
        score: None,
        evidence,
    };
    let pass = |evidence: String, score: Option<f64>| Grade {
        grader: name.clone(),
        pass: true,
        score,
        evidence,
    };
    match spec {
        GraderSpec::ExitCode { expect } => match trial.exit_code {
            Some(code) if code == *expect => pass(format!("exit_code={code}"), None),
            Some(code) => fail(format!("exit_code={code}, expected {expect}")),
            None => fail(format!("no exit code (status {})", trial.status.as_str())),
        },
        GraderSpec::Exact { field, value } => match extract_field(trial, field) {
            Some(v) if value_string(&v) == *value => {
                pass(format!("{field} == {value:?}"), Some(1.0))
            }
            Some(v) => fail(format!(
                "{field} == {:?}, expected {value:?}",
                value_string(&v)
            )),
            None => fail(format!("{field} absent")),
        },
        GraderSpec::Regex { field, pattern } => {
            let re = match regex::Regex::new(pattern) {
                Ok(re) => re,
                Err(e) => return fail(format!("bad pattern {pattern:?}: {e}")),
            };
            match extract_field(trial, field) {
                Some(v) => {
                    let text = value_string(&v);
                    if re.is_match(&text) {
                        pass(format!("{field} matches {pattern:?}"), Some(1.0))
                    } else {
                        fail(format!("{field} == {text:?} does not match {pattern:?}"))
                    }
                }
                None => fail(format!("{field} absent")),
            }
        }
        GraderSpec::NotRegex { field, pattern } => {
            let re = match regex::Regex::new(pattern) {
                Ok(re) => re,
                Err(e) => return fail(format!("bad pattern {pattern:?}: {e}")),
            };
            match extract_field(trial, field) {
                Some(v) => {
                    let text = value_string(&v);
                    if re.is_match(&text) {
                        fail(format!("{field} == {text:?} matches forbidden {pattern:?}"))
                    } else {
                        pass(format!("{field} free of {pattern:?}"), Some(1.0))
                    }
                }
                None => fail(format!("{field} absent")),
            }
        }
        GraderSpec::Range { field, min, max } => match extract_field(trial, field) {
            Some(Value::Number(n)) => {
                let x = n.as_f64().unwrap_or(f64::NAN);
                let lo_ok = min.is_none_or(|lo| x >= lo);
                let hi_ok = max.is_none_or(|hi| x <= hi);
                if lo_ok && hi_ok {
                    pass(format!("{field}={x} in range"), Some(1.0))
                } else {
                    fail(format!(
                        "{field}={x} outside [{}, {}]",
                        min.map_or("-inf".to_string(), |v| v.to_string()),
                        max.map_or("+inf".to_string(), |v| v.to_string()),
                    ))
                }
            }
            Some(v) => fail(format!("{field} == {:?} is not numeric", value_string(&v))),
            None => fail(format!("{field} absent")),
        },
        GraderSpec::JsonSchema { field, schema } => {
            let target = match field {
                Some(f) => extract_field(trial, f),
                None => trial.output.clone(),
            };
            match target {
                Some(v) => {
                    let mut violations = Vec::new();
                    check_schema(&v, schema, "$", &mut violations);
                    if violations.is_empty() {
                        pass("schema valid".to_string(), Some(1.0))
                    } else {
                        fail(format!("schema violations: {}", violations.join("; ")))
                    }
                }
                None => fail(format!(
                    "schema target {} absent",
                    field.as_deref().unwrap_or("output")
                )),
            }
        }
        GraderSpec::Stability => pass("case-level (see stability grade)".to_string(), None),
        GraderSpec::JudgeLite { field, min_score } => match extract_field(trial, field) {
            Some(v) => {
                let text = value_string(&v);
                let score = judge_lite_score(&text);
                if score >= *min_score {
                    pass(
                        format!("judge_lite score {score:.2} >= {min_score:.2}"),
                        Some(score),
                    )
                } else {
                    fail(format!("judge_lite score {score:.2} < {min_score:.2}"))
                }
            }
            None => fail(format!("{field} absent")),
        },
        GraderSpec::FileExists { path } => {
            if files_root.join(path).exists() {
                pass(format!("{path} exists"), Some(1.0))
            } else {
                fail(format!("{path} missing under {}", files_root.display()))
            }
        }
        GraderSpec::FileMatches { path, pattern } => {
            let re = match regex::Regex::new(pattern) {
                Ok(re) => re,
                Err(e) => return fail(format!("bad pattern {pattern:?}: {e}")),
            };
            let full = files_root.join(path);
            match std::fs::read_to_string(&full) {
                Ok(text) if text.len() > 1_048_576 => fail(format!("{path} exceeds 1 MiB cap")),
                Ok(text) if re.is_match(&text) => {
                    pass(format!("{path} matches {pattern:?}"), Some(1.0))
                }
                Ok(_) => fail(format!("{path} does not match {pattern:?}")),
                Err(e) => fail(format!("{path} unreadable: {e}")),
            }
        }
    }
}

/// Grade a case: one `Grade` per grader spec. Non-stability graders must
/// pass on *every* trial; stability compares normalized trial outputs.
pub fn grade_case(spec: &CaseSpec, trials: &[TrialObservation], files_root: &Path) -> Vec<Grade> {
    let mut grades = Vec::new();
    for grader in &spec.graders {
        if matches!(grader, GraderSpec::Stability) {
            grades.push(grade_stability(spec, trials));
            continue;
        }
        if trials.is_empty() {
            grades.push(Grade {
                grader: grader.name(),
                pass: false,
                score: None,
                evidence: "no trials recorded".to_string(),
            });
            continue;
        }
        let per_trial: Vec<Grade> = trials
            .iter()
            .map(|t| grade_one(grader, t, files_root))
            .collect();
        let pass = per_trial.iter().all(|g| g.pass);
        let evidence = if pass {
            format!(
                "{} passed on {}/{} trials",
                grader.name(),
                trials.len(),
                trials.len()
            )
        } else {
            per_trial
                .iter()
                .enumerate()
                .filter(|(_, g)| !g.pass)
                .map(|(i, g)| format!("trial {i}: {}", g.evidence))
                .collect::<Vec<_>>()
                .join("; ")
        };
        grades.push(Grade {
            grader: grader.name(),
            pass,
            score: if pass { Some(1.0) } else { Some(0.0) },
            evidence,
        });
    }
    grades
}

fn grade_stability(spec: &CaseSpec, trials: &[TrialObservation]) -> Grade {
    let name = GraderSpec::Stability.name();
    if trials.len() < 2 {
        return Grade {
            grader: name,
            pass: true,
            score: None,
            evidence: format!("n/a: {} trial(s), stability needs >= 2", trials.len()),
        };
    }
    // Infrastructure observations are never "stable outputs": two
    // identical crashes must not pass. Handled case_error outcomes are
    // domain negatives and stay comparable.
    for (i, trial) in trials.iter().enumerate() {
        if matches!(
            trial.status,
            TrialStatus::Crash | TrialStatus::Timeout | TrialStatus::ProtocolError
        ) {
            return Grade {
                grader: name,
                pass: false,
                score: Some(0.0),
                evidence: format!(
                    "trial {i} observed {} — infrastructure failures are never stable",
                    trial.status.as_str()
                ),
            };
        }
    }
    let first = eval_core::normalize_trial_value(&trials[0]);
    for (i, trial) in trials.iter().enumerate().skip(1) {
        if eval_core::normalize_trial_value(trial) != first {
            return Grade {
                grader: name,
                pass: false,
                score: Some(0.0),
                evidence: format!(
                    "trial {i} differs from trial 0 (status {})",
                    trial.status.as_str()
                ),
            };
        }
    }
    // A nondeterministic case that happens to be stable still passes.
    let _ = spec;
    Grade {
        grader: name,
        pass: true,
        score: Some(1.0),
        evidence: format!("stable across {} trials", trials.len()),
    }
}

/// Deterministic judge stand-in: citations + substantive length.
/// Documented heuristic, never presented as model judgment.
pub fn judge_lite_score(text: &str) -> f64 {
    let citations = text
        .match_indices('[')
        .filter(|(i, _)| {
            text[*i + 1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
        })
        .count()
        .min(4) as f64;
    let length = (text.len().min(400) as f64) / 400.0 * 0.5;
    (0.25 * citations + length).min(1.0)
}

/// Minimal JSON-Schema-subset validator: `type`, `required`,
/// `properties` (recursive), `enum`, `const`. Anything else in the
/// schema document is ignored (documented subset, not silent full support).
pub fn check_schema(value: &Value, schema: &Value, path: &str, out: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if let Some(expected) = obj.get("type").and_then(|t| t.as_str()) {
        let actual = match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let matches = actual == expected || (expected == "number" && actual == "integer");
        if !matches {
            out.push(format!("{path}: type {actual}, expected {expected}"));
            return;
        }
    }
    if let Some(expected) = obj.get("const")
        && value != expected
    {
        out.push(format!("{path}: value != const"));
    }
    if let Some(options) = obj.get("enum").and_then(|e| e.as_array())
        && !options.iter().any(|o| o == value)
    {
        out.push(format!("{path}: value not in enum"));
    }
    if let (Value::Object(map), Some(required)) =
        (value, obj.get("required").and_then(|r| r.as_array()))
    {
        for key in required.iter().filter_map(|k| k.as_str()) {
            if !map.contains_key(key) {
                out.push(format!("{path}: missing required `{key}`"));
            }
        }
    }
    if let (Value::Object(map), Some(props)) =
        (value, obj.get("properties").and_then(|p| p.as_object()))
    {
        for (key, subschema) in props {
            if let Some(sub) = map.get(key) {
                check_schema(sub, subschema, &format!("{path}.{key}"), out);
            }
        }
    }
}

/// Field-level score helper: fraction of trials whose status is `Ok`.
#[allow(dead_code)]
pub fn ok_fraction(trials: &[TrialObservation]) -> f64 {
    if trials.is_empty() {
        return 0.0;
    }
    trials
        .iter()
        .filter(|t| t.status == TrialStatus::Ok)
        .count() as f64
        / trials.len() as f64
}
