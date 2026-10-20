use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

/// Execution mode of the target process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetMode {
    #[default]
    Worker,
    Oneshot,
}

/// How to spawn the target under test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetSpec {
    /// `argv[0]` may be `$PYTHON` (resolved from `EVAL_PYTHON`, else PATH).
    pub argv: Vec<String>,
    #[serde(default)]
    pub mode: TargetMode,
    /// Target's own internal wall budget, when declared. The runner
    /// validates the timeout rule against it.
    #[serde(default)]
    pub budget_ms: Option<u64>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Generic input shape (mirrors the protocol, YAML side).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputSpec {
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub stdin_text: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Per-case limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitsSpec {
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

/// Config-driven grader binding. Project-specific judgments live in
/// suite config; shared mechanics live in `eval-graders`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraderSpec {
    ExitCode {
        expect: i32,
    },
    Exact {
        field: String,
        value: String,
    },
    Regex {
        field: String,
        pattern: String,
    },
    NotRegex {
        field: String,
        pattern: String,
    },
    Range {
        field: String,
        min: Option<f64>,
        max: Option<f64>,
    },
    JsonSchema {
        #[serde(default)]
        field: Option<String>,
        schema: serde_json::Value,
    },
    /// Deterministic judge stand-in: scores text quality without a model.
    /// Live judges stay an explicit opt-in (no network in v1).
    JudgeLite {
        field: String,
        min_score: f64,
    },
    Stability,
    FileExists {
        path: String,
    },
    FileMatches {
        path: String,
        pattern: String,
    },
}

impl GraderSpec {
    /// Stable short name used in grades and reports.
    pub fn name(&self) -> String {
        match self {
            Self::ExitCode { expect } => format!("exit_code[{expect}]"),
            Self::Exact { field, .. } => format!("exact[{field}]"),
            Self::Regex { field, .. } => format!("regex[{field}]"),
            Self::NotRegex { field, .. } => format!("not_regex[{field}]"),
            Self::Range { field, .. } => format!("range[{field}]"),
            Self::JsonSchema { field, .. } => {
                format!("json_schema[{}]", field.as_deref().unwrap_or("output"))
            }
            Self::JudgeLite { field, .. } => format!("judge_lite[{field}]"),
            Self::Stability => "stability".to_string(),
            Self::FileExists { path } => format!("file_exists[{path}]"),
            Self::FileMatches { path, .. } => format!("file_matches[{path}]"),
        }
    }
}

/// One case in a suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseSpec {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub input: InputSpec,
    pub limits: LimitsSpec,
    #[serde(default = "default_trials")]
    pub trials: u32,
    #[serde(default = "default_true")]
    pub deterministic: bool,
    /// Named presets, expanded into `graders` at load time.
    #[serde(default)]
    pub presets: Vec<PresetSpec>,
    #[serde(default)]
    pub graders: Vec<GraderSpec>,
}

fn default_trials() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// Suite-level gate policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatePolicy {
    #[serde(default = "default_min_pass")]
    pub min_pass_rate: f64,
    #[serde(default = "default_delta_floor")]
    pub delta_floor: f64,
    #[serde(default = "default_bootstrap_reps")]
    pub bootstrap_reps: u32,
    #[serde(default = "default_confidence_z")]
    pub confidence_z: f64,
}

fn default_min_pass() -> f64 {
    1.0
}

fn default_delta_floor() -> f64 {
    0.05
}

fn default_bootstrap_reps() -> u32 {
    1000
}

fn default_confidence_z() -> f64 {
    1.959963984540054
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self {
            min_pass_rate: default_min_pass(),
            delta_floor: default_delta_floor(),
            bootstrap_reps: default_bootstrap_reps(),
            confidence_z: default_confidence_z(),
        }
    }
}

/// Named grader preset, or a parameterized one (`{json_object: {required: [...]}}`).
///
/// Presets expand to ordinary [`GraderSpec`]s at load time, so everything
/// downstream (validation, grading, reports) sees plain graders. They exist
/// to cut per-suite boilerplate and the class of errors that comes with
/// hand-writing the same five grader blocks for every case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PresetSpec {
    /// `- cli_contract`
    Named(String),
    /// `- json_object: {required: [a, b]}`
    Parameterized(std::collections::BTreeMap<String, serde_json::Value>),
}

/// The preset catalogue an agent can discover via `eval schema`.
pub const PRESET_NAMES: &[&str] = &[
    "cli_contract",
    "exit_zero",
    "stable",
    "nonempty_output",
    "no_secret_leak",
    "json_object",
];

/// Expand one preset into graders. Unknown names are a config error naming
/// the valid set (fail fast, never silently ignore).
pub fn resolve_preset(spec: &PresetSpec, case_id: &str) -> Result<Vec<GraderSpec>, CoreError> {
    let named = |name: &str| -> Vec<GraderSpec> {
        match name {
            "cli_contract" | "exit_zero" => vec![GraderSpec::ExitCode { expect: 0 }],
            "stable" => vec![GraderSpec::Stability],
            "nonempty_output" => vec![GraderSpec::Regex {
                field: "output.text".to_string(),
                pattern: r"\S".to_string(),
            }],
            "no_secret_leak" => vec![GraderSpec::NotRegex {
                field: "output.text".to_string(),
                pattern: r"(?i)(authorization\s*:|bearer\s+[A-Za-z0-9._-]+|api[_-]?key\s*[:=]|sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16})".to_string(),
            }],
            _ => Vec::new(),
        }
    };
    match spec {
        PresetSpec::Named(name) => {
            let graders = named(name);
            if graders.is_empty() && name != "json_object" {
                return Err(CoreError::Config(format!(
                    "case `{case_id}`: unknown grader preset `{name}`; valid presets: {}",
                    PRESET_NAMES.join(", ")
                )));
            }
            Ok(graders)
        }
        PresetSpec::Parameterized(map) => {
            let (name, params) = map
                .iter()
                .next()
                .ok_or_else(|| CoreError::Config(format!("case `{case_id}`: empty preset")))?;
            match name.as_str() {
                "json_object" => {
                    let field = params
                        .get("field")
                        .and_then(|v| v.as_str())
                        .unwrap_or("output.data")
                        .to_string();
                    let required = params
                        .get("required")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let mut schema = serde_json::Map::new();
                    schema.insert("type".to_string(), serde_json::json!("object"));
                    if !required.is_empty() {
                        schema.insert("required".to_string(), serde_json::json!(required));
                    }
                    Ok(vec![GraderSpec::JsonSchema {
                        field: Some(field),
                        schema: serde_json::Value::Object(schema),
                    }])
                }
                other => Err(CoreError::Config(format!(
                    "case `{case_id}`: unknown grader preset `{other}`; valid presets: {}",
                    PRESET_NAMES.join(", ")
                ))),
            }
        }
    }
}

/// A versioned evaluation suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    pub suite: String,
    pub version: u32,
    #[serde(default)]
    pub target: Option<TargetSpec>,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub gate: GatePolicy,
    pub cases: Vec<CaseSpec>,
}

impl Suite {
    /// Load and validate a suite YAML file.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CoreError::io(path.display().to_string(), e))?;
        let mut suite: Self =
            serde_yaml::from_str(&text).map_err(|e| CoreError::Config(e.to_string()))?;
        suite.expand_presets()?;
        suite.validate()?;
        Ok(suite)
    }

    /// Expand `presets` into concrete graders (presets first, then any
    /// explicit `graders`). After this the suite carries only plain
    /// [`GraderSpec`]s, so nothing downstream needs to know about presets.
    pub fn expand_presets(&mut self) -> Result<(), CoreError> {
        for case in &mut self.cases {
            if case.presets.is_empty() {
                continue;
            }
            let mut expanded = Vec::new();
            for preset in &case.presets {
                expanded.extend(resolve_preset(preset, &case.id)?);
            }
            expanded.append(&mut case.graders);
            case.graders = expanded;
            case.presets.clear();
        }
        Ok(())
    }

    /// Structural validation at suite load (fail fast, exit 2 later).
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.suite.trim().is_empty() {
            return Err(CoreError::Config("suite id must be non-empty".to_string()));
        }
        if self.cases.is_empty() {
            return Err(CoreError::Config(
                "suite must declare >= 1 case".to_string(),
            ));
        }
        let mut seen = HashSet::new();
        for case in &self.cases {
            if case.id.trim().is_empty() {
                return Err(CoreError::Config("case id must be non-empty".to_string()));
            }
            if !seen.insert(case.id.as_str()) {
                return Err(CoreError::Config(format!(
                    "duplicate case id `{}`",
                    case.id
                )));
            }
            if case.limits.timeout_ms < 1 {
                return Err(CoreError::Config(format!(
                    "case `{}`: limits.timeout_ms must be >= 1",
                    case.id
                )));
            }
            if case.limits.max_output_bytes < 1 {
                return Err(CoreError::Config(format!(
                    "case `{}`: limits.max_output_bytes must be >= 1",
                    case.id
                )));
            }
            if case.trials < 1 {
                return Err(CoreError::Config(format!(
                    "case `{}`: trials must be >= 1",
                    case.id
                )));
            }
            if !case.deterministic && case.trials < 2 {
                return Err(CoreError::Config(format!(
                    "case `{}`: sampling cases need trials >= 2",
                    case.id
                )));
            }
            for grader in &case.graders {
                // Nested (not let-chains): MSRV 1.85.
                if let GraderSpec::Range { min, max, .. } = grader {
                    if let (Some(lo), Some(hi)) = (min, max) {
                        if lo > hi {
                            return Err(CoreError::Config(format!(
                                "case `{}`: range min > max",
                                case.id
                            )));
                        }
                    }
                }
            }
        }
        if let Some(target) = &self.target {
            if let Some(budget) = target.budget_ms {
                if budget > 0 {
                    for case in &self.cases {
                        let need = (budget as f64) * crate::record::TIMEOUT_RULE_FACTOR;
                        if (case.limits.timeout_ms as f64) < need {
                            return Err(CoreError::Config(format!(
                                "case `{}`: timeout {}ms violates the 1.5x rule against target budget {}ms",
                                case.id, case.limits.timeout_ms, budget
                            )));
                        }
                    }
                }
            }
        }
        if !(0.0..=1.0).contains(&self.gate.min_pass_rate) {
            return Err(CoreError::Config(
                "gate.min_pass_rate must be in [0, 1]".to_string(),
            ));
        }
        Ok(())
    }
}
