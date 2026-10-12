use crate::error::UsageError;
use crate::event::{LlmEvent, EVENT_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Append-only, idempotent file store for usage events.
///
/// Layout under the store root:
/// `usage/<project>/events-YYYYMMDD.jsonl` (one event per line),
/// `usage/<project>/seen_ids.txt` (idempotency set, one id per line),
/// `usage/<project>/rejected-YYYYMMDD.jsonl` (per-line rejections).
///
/// Reprocessing the same file yields all-duplicates and changes
/// nothing. History is never rewritten.
pub struct UsageStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejection {
    pub line: usize,
    pub reason: String,
    pub raw: String,
}

/// Outcome of one import batch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportSummary {
    pub accepted: usize,
    pub duplicates: usize,
    pub rejected: Vec<Rejection>,
}

impl UsageStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn project_dir(&self, project: &str) -> PathBuf {
        self.root.join("usage").join(project)
    }

    fn date_stamp() -> String {
        // Days since epoch is enough for file rotation; readable dates
        // are rendered at display time.
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0);
        format!("d{days}")
    }

    fn read_lines(path: &Path) -> Result<Vec<String>, UsageError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(text.lines().map(str::to_string).collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(UsageError::io(path.display().to_string(), e)),
        }
    }

    fn append_line(path: &Path, line: &str) -> Result<(), UsageError> {
        use std::io::Write as _;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| UsageError::io(parent.display().to_string(), e))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| UsageError::io(path.display().to_string(), e))?;
        writeln!(file, "{line}").map_err(|e| UsageError::io(path.display().to_string(), e))
    }

    /// Idempotency set: the persisted index UNION ids scanned from the
    /// append-only log. The union makes recovery crash-safe in both
    /// directions — an event written without its index entry (crash
    /// between the two appends) is still recognized on redelivery,
    /// and an index entry without its event can never suppress a
    /// genuinely new event (ids are content keys, never reused).
    fn seen_ids(&self, project: &str) -> Result<HashSet<String>, UsageError> {
        let mut seen: HashSet<String> = Self::read_lines(&self.project_dir(project).join("seen_ids.txt"))?
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let (events, _) = self.read_events(project)?;
        for event in events {
            seen.insert(event.event_id);
        }
        Ok(seen)
    }

    /// Import one JSONL batch. `tag` stamps events that lack one
    /// (benchmark/variant labeling); explicit per-event tags win. Same
    /// for `run_id`: stamps the controlled run the batch belongs to so
    /// usage joins grades later. Neither rewrites history — stamps apply
    /// to this batch only.
    pub fn import_str(
        &self,
        project: &str,
        body: &str,
        tag: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<ImportSummary, UsageError> {
        if project.trim().is_empty() {
            return Err(UsageError::Config("project must be non-empty".to_string()));
        }
        let mut seen = self.seen_ids(project)?;
        let mut summary = ImportSummary::default();
        let stamp = Self::date_stamp();
        let events_path = self.project_dir(project).join(format!("events-{stamp}.jsonl"));
        let rejected_path = self.project_dir(project).join(format!("rejected-{stamp}.jsonl"));
        for (index, line) in body.lines().enumerate() {
            let line_no = index + 1;
            if line.trim().is_empty() {
                continue;
            }
            let mut event: LlmEvent = match serde_json::from_str(line) {
                Ok(event) => event,
                Err(e) => {
                    summary.rejected.push(Rejection {
                        line: line_no,
                        reason: format!("invalid JSON: {e}"),
                        raw: truncate(line, 500),
                    });
                    continue;
                }
            };
            if event.event_version != EVENT_VERSION {
                summary.rejected.push(Rejection {
                    line: line_no,
                    reason: format!(
                        "unsupported event version {} (reader supports llm/v{EVENT_VERSION})",
                        event.event_version
                    ),
                    raw: truncate(line, 500),
                });
                continue;
            }
            // Project binding: the import target wins on conflict, and
            // the conflict is recorded rather than silently kept.
            if event.project != project {
                summary.rejected.push(Rejection {
                    line: line_no,
                    reason: format!(
                        "project mismatch: event says {:?}, import targets {:?}",
                        event.project, project
                    ),
                    raw: truncate(line, 500),
                });
                continue;
            }
            if event.tag.is_none() {
                event.tag = tag.map(str::to_string);
            }
            if event.run_id.is_none() {
                event.run_id = run_id.map(str::to_string);
            }
            match crate::event::validate_event(&event) {
                Ok(()) => {}
                Err(UsageError::Rejected { reason, .. }) => {
                    summary.rejected.push(Rejection {
                        line: line_no,
                        reason,
                        raw: truncate(line, 500),
                    });
                    continue;
                }
                Err(e) => {
                    summary.rejected.push(Rejection {
                        line: line_no,
                        reason: e.to_string(),
                        raw: truncate(line, 500),
                    });
                    continue;
                }
            }
            if !seen.insert(event.event_id.clone()) {
                summary.duplicates += 1;
                continue;
            }
            let canonical = serde_json::to_string(&event)
                .map_err(|e| UsageError::Config(format!("serialize: {e}")))?;
            Self::append_line(&events_path, &canonical)?;
            Self::append_line(
                &self.project_dir(project).join("seen_ids.txt"),
                &event.event_id,
            )?;
            summary.accepted += 1;
        }
        if !summary.rejected.is_empty() {
            for rejection in &summary.rejected {
                let line = serde_json::to_string(rejection)
                    .map_err(|e| UsageError::Config(format!("serialize: {e}")))?;
                Self::append_line(&rejected_path, &line)?;
            }
        }
        Ok(summary)
    }

    /// Import a JSONL file.
    pub fn import_file(
        &self,
        project: &str,
        path: &Path,
        tag: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<ImportSummary, UsageError> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| UsageError::io(path.display().to_string(), e))?;
        self.import_str(project, &body, tag, run_id)
    }

    /// All accepted events for a project, in stored order.
    /// Corrupt stored lines are skipped with a count (storage is
    /// append-only; a bad line never blocks reads).
    pub fn read_events(&self, project: &str) -> Result<(Vec<LlmEvent>, usize), UsageError> {
        let dir = self.project_dir(project);
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name().is_some_and(|n| {
                        n.to_string_lossy().starts_with("events-")
                            && p.extension().is_some_and(|x| x == "jsonl")
                    })
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(e) => return Err(UsageError::io(dir.display().to_string(), e)),
        };
        files.sort();
        let mut events = Vec::new();
        let mut skipped = 0;
        for file in files {
            for line in Self::read_lines(&file)? {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<LlmEvent>(&line) {
                    Ok(event) => events.push(event),
                    Err(_) => skipped += 1,
                }
            }
        }
        Ok((events, skipped))
    }

    /// Registered project ids (directories under `usage/`).
    pub fn projects(&self) -> Result<Vec<String>, UsageError> {
        let dir = self.root.join("usage");
        match std::fs::read_dir(&dir) {
            Ok(entries) => Ok(entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(UsageError::io(dir.display().to_string(), e)),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…<truncated>", &s[..max])
    }
}
