use crate::error::CoreError;
use crate::record::{RunRecord, SCHEMA_VERSION, TrialObservation};
use crate::run_manifest::RunManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Pointer to the accepted baseline run of a suite (never a copy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselinePointer {
    pub suite: String,
    pub ref_name: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBackfillReport {
    pub created: usize,
    pub existing: usize,
    pub failed: Vec<String>,
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

    fn write_once(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::io(parent.display().to_string(), e))?;
        }
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = std::fs::read(path)
                    .map_err(|e| CoreError::io(path.display().to_string(), e))?;
                if existing == bytes {
                    return Ok(());
                }
                return Err(CoreError::Config(format!(
                    "immutable file {} already exists with different content",
                    path.display()
                )));
            }
            Err(e) => return Err(CoreError::io(path.display().to_string(), e)),
        };
        use std::io::Write as _;
        file.write_all(bytes)
            .map_err(|e| CoreError::io(path.display().to_string(), e))?;
        file.sync_all()
            .map_err(|e| CoreError::io(path.display().to_string(), e))
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
        // The id is content-addressed; the id field itself must not feed
        // back into the hash, or re-saving a loaded record changes its id.
        redacted.run_id.clear();
        let canonical =
            serde_json::to_vec(&redacted).map_err(|e| CoreError::Json(e.to_string()))?;
        let run_id = crate::ids::new_run_id(redacted.started_at_ms, &canonical);
        redacted.run_id = run_id.clone();
        *record = redacted.clone();

        let metadata =
            serde_json::to_string_pretty(&redacted).map_err(|e| CoreError::Json(e.to_string()))?;
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
        let grades =
            serde_json::to_string_pretty(&grades).map_err(|e| CoreError::Json(e.to_string()))?;
        let decision = match &redacted.gate {
            Some(gate) => Some(
                serde_json::to_string_pretty(gate).map_err(|e| CoreError::Json(e.to_string()))?,
            ),
            None => None,
        };

        let dir = self.run_dir(&redacted.suite, &redacted.ref_name, &run_id);
        if dir.exists() {
            Self::write_once(&dir.join("metadata.json"), metadata.as_bytes())?;
            Self::write_once(&dir.join("observations.jsonl"), obs_lines.as_bytes())?;
            Self::write_once(&dir.join("grades.json"), grades.as_bytes())?;
            if let Some(decision) = &decision {
                Self::write_once(&dir.join("decision.json"), decision.as_bytes())?;
            }
            return Ok(run_id);
        }

        let parent = dir
            .parent()
            .ok_or_else(|| CoreError::Config("run directory has no parent".to_string()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::io(parent.display().to_string(), e))?;
        static STAGING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = STAGING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "run".to_string());
        let staging = parent.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)
                .map_err(|e| CoreError::io(staging.display().to_string(), e))?;
        }
        let staged = (|| -> Result<(), CoreError> {
            std::fs::create_dir_all(&staging)
                .map_err(|e| CoreError::io(staging.display().to_string(), e))?;
            Self::write(&staging.join("observations.jsonl"), obs_lines.as_bytes())?;
            Self::write(&staging.join("grades.json"), grades.as_bytes())?;
            if let Some(decision) = &decision {
                Self::write(&staging.join("decision.json"), decision.as_bytes())?;
            }
            Self::write(&staging.join("metadata.json"), metadata.as_bytes())
        })();
        if let Err(e) = staged {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&staging, &dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(CoreError::io(dir.display().to_string(), e));
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
    /// Find a stored run carrying a caller idempotency key. Used by
    /// `eval watch` to close the crash window between storing a run and
    /// persisting its queue state.
    pub fn find_request_id(&self, suite: &str, request_id: &str) -> Option<RunRecord> {
        let refs = std::fs::read_dir(self.root.join("runs").join(suite)).ok()?;
        for ref_entry in refs.flatten() {
            let runs = match std::fs::read_dir(ref_entry.path()) {
                Ok(runs) => runs,
                Err(_) => continue,
            };
            for run_entry in runs.flatten() {
                let metadata = run_entry.path().join("metadata.json");
                let text = match std::fs::read_to_string(metadata) {
                    Ok(text) => text,
                    Err(_) => continue,
                };
                let record: RunRecord = match serde_json::from_str(&text) {
                    Ok(record) => record,
                    Err(_) => continue,
                };
                if record.request_id.as_deref() == Some(request_id) {
                    return Some(record);
                }
            }
        }
        None
    }

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

    /// Optional `manifest.json` sidecar for a run. Missing is `None`;
    /// unreadable or invalid manifests are errors, never silent.
    pub fn load_manifest(
        &self,
        suite: &str,
        ref_name: &str,
        run_id: &str,
    ) -> Result<Option<RunManifest>, CoreError> {
        let path = self.run_dir(suite, ref_name, run_id).join("manifest.json");
        if !path.exists() {
            return Ok(None);
        }
        let manifest = RunManifest::load(&path)?;
        if manifest.run_id != run_id {
            return Err(CoreError::Config(format!(
                "manifest run_id {} does not match directory {run_id}",
                manifest.run_id
            )));
        }
        if manifest.suite.id != suite || manifest.ref_name != ref_name {
            return Err(CoreError::Config(format!(
                "manifest identity {}/{} does not match {suite}/{ref_name}",
                manifest.suite.id, manifest.ref_name
            )));
        }
        Ok(Some(manifest))
    }

    /// Create `manifest.json` for every run that lacks one. Legacy runs
    /// keep lane/free-only/model/target as `null` (n/a): backfill never
    /// invents provenance. Existing manifests are left untouched.
    pub fn backfill_manifests(&self) -> Result<ManifestBackfillReport, CoreError> {
        let mut report = ManifestBackfillReport::default();
        let runs_dir = self.root.join("runs");
        let suites = match std::fs::read_dir(&runs_dir) {
            Ok(suites) => suites,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(e) => return Err(CoreError::io(runs_dir.display().to_string(), e)),
        };
        for suite in suites.filter_map(|entry| entry.ok()) {
            let suite_name = suite.file_name().to_string_lossy().into_owned();
            let refs = match std::fs::read_dir(suite.path()) {
                Ok(refs) => refs,
                Err(e) => {
                    report.failed.push(format!("{suite_name}: {e}"));
                    continue;
                }
            };
            for ref_dir in refs.filter_map(|entry| entry.ok()) {
                let ref_name = ref_dir.file_name().to_string_lossy().into_owned();
                let runs = match std::fs::read_dir(ref_dir.path()) {
                    Ok(runs) => runs,
                    Err(e) => {
                        report.failed.push(format!("{suite_name}/{ref_name}: {e}"));
                        continue;
                    }
                };
                for run in runs.filter_map(|entry| entry.ok()) {
                    let dir_name = run.file_name().to_string_lossy().into_owned();
                    let Some(run_id) = dir_name.strip_prefix("run-") else {
                        continue;
                    };
                    let label = format!("{suite_name}/{ref_name}/{run_id}");
                    if run.path().join("manifest.json").exists() {
                        match self.load_manifest(&suite_name, &ref_name, run_id) {
                            Ok(_) => report.existing += 1,
                            Err(e) => report.failed.push(format!("{label}: {e}")),
                        }
                        continue;
                    }
                    let record = match self.load(&suite_name, &ref_name, run_id) {
                        Ok(record) => record,
                        Err(e) => {
                            report.failed.push(format!("{label}: {e}"));
                            continue;
                        }
                    };
                    let manifest = RunManifest::from_record(
                        &record,
                        None,
                        Some(record.code_version.clone()),
                        None,
                        None,
                        None,
                    );
                    match self.save_manifest(&suite_name, &ref_name, run_id, &manifest) {
                        Ok(()) => report.created += 1,
                        Err(e) => report.failed.push(format!("{label}: {e}")),
                    }
                }
            }
        }
        Ok(report)
    }

    /// Write the manifest sidecar once. Identical rewrites are idempotent;
    /// a different manifest for an existing run is rejected.
    pub fn save_manifest(
        &self,
        suite: &str,
        ref_name: &str,
        run_id: &str,
        manifest: &RunManifest,
    ) -> Result<(), CoreError> {
        if manifest.run_id != run_id {
            return Err(CoreError::Config(format!(
                "manifest run_id {} does not match {run_id}",
                manifest.run_id
            )));
        }
        if manifest.suite.id != suite || manifest.ref_name != ref_name {
            return Err(CoreError::Config(format!(
                "manifest identity {}/{} does not match {suite}/{ref_name}",
                manifest.suite.id, manifest.ref_name
            )));
        }
        manifest.validate()?;
        let dir = self.run_dir(suite, ref_name, run_id);
        if !dir.join("metadata.json").exists() {
            return Err(CoreError::NotFound(dir.display().to_string()));
        }
        let path = dir.join("manifest.json");
        let body = manifest.to_json_pretty()?;
        Self::write_once(&path, body.as_bytes())
    }
}
