use crate::error::CoreError;
use crate::record::{RunRecord, SCHEMA_VERSION, TrialObservation};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Pointer to the accepted baseline run of a suite (never a copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselinePointer {
    pub suite: String,
    pub ref_name: String,
    pub run_id: String,
}

/// File-backed run store.
///
/// Layout (relative to the store root):
/// `runs/<suite>/<ref>/run-<run_id>/metadata.json`,
/// `observations.jsonl`, `grades.json`, `decision.json` (optional),
/// and `baselines/<suite>.json` (pointer, not copy).
pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run_dir(&self, suite: &str, ref_name: &str, run_id: &str) -> PathBuf {
        self.root
            .join("runs")
            .join(suite)
            .join(ref_name)
            .join(format!("run-{run_id}"))
    }

    fn baseline_path(&self, suite: &str) -> PathBuf {
        self.root.join("baselines").join(format!("{suite}.json"))
    }

    fn write(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::io(parent.display().to_string(), e))?;
        }
        std::fs::write(path, bytes).map_err(|e| CoreError::io(path.display().to_string(), e))
    }

    fn read(path: &Path) -> Result<String, CoreError> {
        std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CoreError::NotFound(path.display().to_string())
            } else {
                CoreError::io(path.display().to_string(), e)
            }
        })
    }

    /// Persist a run. The record is redacted before hashing so the run
    /// id is stable and secret-free. Returns the run id.
    pub fn save(
        &self,
        record: &mut RunRecord,
        observations: &[TrialObservation],
    ) -> Result<String, CoreError> {
        record.schema_version = SCHEMA_VERSION;
        let mut redacted = record.clone();
        let body = serde_json::to_value(&redacted).map_err(|e| CoreError::Json(e.to_string()))?;
        let body = crate::redact::redact_json(&body);
        redacted = serde_json::from_value(body).map_err(|e| CoreError::Json(e.to_string()))?;
        let canonical =
            serde_json::to_vec(&redacted).map_err(|e| CoreError::Json(e.to_string()))?;
        let run_id = crate::ids::new_run_id(redacted.started_at_ms, &canonical);
        redacted.run_id = run_id.clone();
        *record = redacted.clone();

        let dir = self.run_dir(&redacted.suite, &redacted.ref_name, &run_id);
        Self::write(
            &dir.join("metadata.json"),
            serde_json::to_string_pretty(&redacted)
                .map_err(|e| CoreError::Json(e.to_string()))?
                .as_bytes(),
        )?;
        let mut obs_lines = String::new();
        for trial in observations {
            let redacted_trial = crate::redact::redact_json(
                &serde_json::to_value(trial).map_err(|e| CoreError::Json(e.to_string()))?,
            );
            obs_lines.push_str(
                &serde_json::to_string(&redacted_trial)
                    .map_err(|e| CoreError::Json(e.to_string()))?,
            );
            obs_lines.push('\n');
        }
        Self::write(&dir.join("observations.jsonl"), obs_lines.as_bytes())?;
        let grades = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "run_id": run_id,
            "cases": redacted.cases.iter().map(|c| serde_json::json!({
                "case_id": c.case_id,
                "pass": c.pass,
                "digest": c.digest,
                "grades": c.grades,
            })).collect::<Vec<_>>(),
        });
        Self::write(
            &dir.join("grades.json"),
            serde_json::to_string_pretty(&grades)
                .map_err(|e| CoreError::Json(e.to_string()))?
                .as_bytes(),
        )?;
        if let Some(gate) = &redacted.gate {
            Self::write(
                &dir.join("decision.json"),
                serde_json::to_string_pretty(gate)
                    .map_err(|e| CoreError::Json(e.to_string()))?
                    .as_bytes(),
            )?;
        }
        Ok(run_id)
    }

    /// Load a run and reassemble the full record.
    pub fn load(&self, suite: &str, ref_name: &str, run_id: &str) -> Result<RunRecord, CoreError> {
        let dir = self.run_dir(suite, ref_name, run_id);
        let text = Self::read(&dir.join("metadata.json"))?;
        let mut record: RunRecord =
            serde_json::from_str(&text).map_err(|e| CoreError::Json(e.to_string()))?;
        if record.schema_version > SCHEMA_VERSION {
            return Err(CoreError::Config(format!(
                "run schema v{} newer than reader v{SCHEMA_VERSION}",
                record.schema_version
            )));
        }
        let obs_text = Self::read(&dir.join("observations.jsonl"))?;
        let mut by_case: std::collections::HashMap<String, Vec<TrialObservation>> =
            std::collections::HashMap::new();
        for line in obs_text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let trial: TrialObservation =
                serde_json::from_str(line).map_err(|e| CoreError::Json(e.to_string()))?;
            by_case
                .entry(trial.case_id.clone())
                .or_default()
                .push(trial);
        }
        for case in &mut record.cases {
            if let Some(trials) = by_case.remove(&case.case_id) {
                case.trials = trials;
            }
        }
        if let Ok(decision_text) = std::fs::read_to_string(dir.join("decision.json")) {
            record.gate = serde_json::from_str(&decision_text).ok();
        }
        Ok(record)
    }

    /// Newest run for a suite ref, or `None`.
    pub fn latest(&self, suite: &str, ref_name: &str) -> Result<Option<String>, CoreError> {
        let dir = self.root.join("runs").join(suite).join(ref_name);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CoreError::io(dir.display().to_string(), e)),
        };
        let mut ids: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("run-"))
            .map(|n| n.trim_start_matches("run-").to_string())
            .collect();
        ids.sort();
        Ok(ids.pop())
    }

    /// Set the baseline pointer for a suite.
    pub fn set_baseline(&self, pointer: &BaselinePointer) -> Result<(), CoreError> {
        Self::write(
            &self.baseline_path(&pointer.suite),
            serde_json::to_string_pretty(pointer)
                .map_err(|e| CoreError::Json(e.to_string()))?
                .as_bytes(),
        )
    }

    /// Read the baseline pointer for a suite.
    pub fn baseline(&self, suite: &str) -> Result<BaselinePointer, CoreError> {
        let text = Self::read(&self.baseline_path(suite))?;
        serde_json::from_str(&text).map_err(|e| CoreError::Json(e.to_string()))
    }
}
