//! Deterministic integration validator.
//!
//! `eval check` runs this. It is the agent's iteration gate: a single
//! command that returns pass/fail with actionable, machine-readable
//! issues, so an agent can build a suite and converge without reading
//! prose or guessing. Every issue carries a stable `code` and a `hint`.

use eval_core::{GraderSpec, Suite};
use serde::{Deserialize, Serialize};

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding. `code` is stable and greppable; `hint` says how to fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub severity: Severity,
    pub code: String,
    pub location: String,
    pub message: String,
    pub hint: String,
}

impl Issue {
    fn error(
        code: &str,
        location: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_string(),
            location: location.into(),
            message: message.into(),
            hint: hint.into(),
        }
    }

    fn warning(
        code: &str,
        location: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.to_string(),
            location: location.into(),
            message: message.into(),
            hint: hint.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Result of validating a suite (and optionally its graders).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub suite: String,
    pub cases: usize,
    pub issues: Vec<Issue>,
}

impl CheckReport {
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.is_error()).count()
    }

    pub fn ok(&self) -> bool {
        self.errors() == 0
    }
}

/// Validate a loaded suite's graders and shape. The suite is assumed to
/// have passed `Suite::validate` already (structural load errors are the
/// caller's to surface). This catches the things graders get wrong.
pub fn check_suite(suite: &Suite) -> CheckReport {
    let mut issues = Vec::new();

    if let Some(target) = &suite.target {
        if target.argv.is_empty() {
            issues.push(Issue::error(
                "target.empty_argv",
                "target.argv",
                "target argv is empty",
                "set target.argv to the command that starts your adapter, e.g. [\"$PYTHON\", \"integrator/adapter.py\"]",
            ));
        }
        if let Some(declared) = target.argv.first() {
            let literal = !declared.starts_with('$');
            if literal && !declared.contains(['/', '\\']) && declared.contains('.') {
                issues.push(Issue::warning(
                    "target.relative_script",
                    "target.argv[0]",
                    format!("`{declared}` looks like a script path but has no directory separator"),
                    "use `$PYTHON` (or an absolute path) for the interpreter and a relative path for the script",
                ));
            }
        }
    } else {
        issues.push(Issue::warning(
            "target.missing",
            "target",
            "suite has no target block",
            "add a `target: { argv: [...], mode: oneshot }` block so the suite is runnable",
        ));
    }

    for case in &suite.cases {
        let loc = |s: &str| format!("cases[{}].{}", case.id, s);
        if case.graders.is_empty() {
            issues.push(Issue::warning(
                "case.no_graders",
                loc("graders"),
                "case declares no graders, so it can only be observed, never graded",
                "add at least one grader (e.g. exit_code, json_schema, regex) or remove the case",
            ));
        }
        let has_stability = case
            .graders
            .iter()
            .any(|g| matches!(g, GraderSpec::Stability));
        // A deterministic case may legitimately run >1 trial when a
        // stability grader is present (that is how stability is checked).
        if case.deterministic && case.trials > 1 && !has_stability {
            issues.push(Issue::warning(
                "case.deterministic_multi_trial",
                loc("trials"),
                "deterministic case runs multiple trials without a stability grader",
                "add a `stability` grader to justify the extra trials, or set trials: 1",
            ));
        }
        if has_stability && case.trials < 2 {
            issues.push(Issue::error(
                "grader.stability_needs_trials",
                loc("graders"),
                "stability grader needs trials >= 2",
                "set trials: 2 (or more) on this case",
            ));
        }
        for (gi, grader) in case.graders.iter().enumerate() {
            let gl = format!("cases[{}].graders[{gi}] ({})", case.id, grader.name());
            if let Some(issue) = check_grader(grader, &gl) {
                issues.push(issue);
            }
        }
    }

    CheckReport {
        suite: suite.suite.clone(),
        cases: suite.cases.len(),
        issues,
    }
}

fn check_grader(grader: &GraderSpec, location: &str) -> Option<Issue> {
    match grader {
        GraderSpec::Regex { field, pattern } | GraderSpec::NotRegex { field, pattern } => {
            if let Err(e) = regex::Regex::new(pattern) {
                return Some(Issue::error(
                    "grader.bad_regex",
                    location,
                    format!("pattern {pattern:?} does not compile: {e}"),
                    "fix the regex; remember YAML double-quoted strings need \\\\ for a literal backslash",
                ));
            }
            if field.trim().is_empty() {
                return Some(Issue::error(
                    "grader.empty_field",
                    location,
                    "field path is empty",
                    "use a field path like output.text, events[0].name, or metrics.wall_ms",
                ));
            }
            None
        }
        GraderSpec::Exact { field, .. } => check_field(field, location),
        GraderSpec::Range { field, min, max } => {
            if let (Some(lo), Some(hi)) = (min, max)
                && lo > hi
            {
                return Some(Issue::error(
                    "grader.range_inverted",
                    location,
                    format!("min {lo} is greater than max {hi}"),
                    "swap the bounds",
                ));
            }
            check_field(field, location)
        }
        GraderSpec::JsonSchema { schema, .. } => {
            if !schema.is_object() {
                return Some(Issue::error(
                    "grader.schema_not_object",
                    location,
                    "json_schema `schema` must be a JSON object",
                    "provide a subset schema: {type: object, required: [..], properties: {..}}",
                ));
            }
            if schema.get("type").is_none() && schema.get("required").is_none() {
                return Some(Issue::warning(
                    "grader.schema_empty",
                    location,
                    "json_schema has neither `type` nor `required`, so it accepts almost anything",
                    "add `type: object` and the keys you actually require",
                ));
            }
            None
        }
        GraderSpec::FileExists { path } => check_path(path, location),
        GraderSpec::FileMatches { path, pattern, .. } => {
            if let Err(e) = regex::Regex::new(pattern) {
                return Some(Issue::error(
                    "grader.bad_regex",
                    location,
                    format!("file pattern {pattern:?} does not compile: {e}"),
                    "fix the regex",
                ));
            }
            check_path(path, location)
        }
        GraderSpec::JudgeLite { min_score, .. } => {
            if !(0.0..=1.0).contains(min_score) {
                return Some(Issue::error(
                    "grader.judge_score_range",
                    location,
                    format!("min_score {min_score} is outside [0, 1]"),
                    "judge_lite scores are in [0, 1]",
                ));
            }
            None
        }
        GraderSpec::ExitCode { .. } | GraderSpec::Stability => None,
    }
}

fn check_field(field: &str, location: &str) -> Option<Issue> {
    if field.trim().is_empty() {
        return Some(Issue::error(
            "grader.empty_field",
            location,
            "field path is empty",
            "use a field path like output.text, events[0].name, or metrics.wall_ms",
        ));
    }
    let known_root = [
        "output",
        "metrics",
        "ai",
        "events",
        "status",
        "exit_code",
        "wall_ms",
        "stderr",
        "error_code",
        "error_message",
    ];
    let root = field.split(['.', '[']).next().unwrap_or("");
    if !known_root.contains(&root) {
        return Some(Issue::warning(
            "grader.unknown_field_root",
            location,
            format!("field root `{root}` is not a known observation path"),
            "known roots: output, metrics, ai, events, status, exit_code, wall_ms, stderr, error_code, error_message",
        ));
    }
    None
}

fn check_path(path: &str, location: &str) -> Option<Issue> {
    if path.trim().is_empty() {
        return Some(Issue::error(
            "grader.empty_path",
            location,
            "artifact path is empty",
            "use a path relative to the directory you run eval from",
        ));
    }
    None
}
