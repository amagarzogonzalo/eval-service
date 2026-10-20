//! PR and baseline promotion policy.
//!
//! A run only acquires provenance when the caller supplies it (`eval run
//! --origin ...`). Missing provenance fails closed for promotion; replay
//! provenance never becomes a live baseline.

use anyhow::{Context, Result, bail};
use eval_core::{BaselinePointer, GateDecision, RunStore, Suite};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const PROVENANCE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Live,
    Replay,
}

impl Origin {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "live" => Ok(Self::Live),
            "replay" => Ok(Self::Replay),
            other => bail!("origin must be live or replay, got {other:?}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Replay => "replay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default = "provenance_schema")]
    pub schema_version: u32,
    pub origin: Origin,
    pub source_ref: String,
    pub event: String,
    pub sandboxed: bool,
}

fn provenance_schema() -> u32 {
    PROVENANCE_SCHEMA_VERSION
}

impl Provenance {
    pub fn new(
        origin: Origin,
        source_ref: impl Into<String>,
        event: impl Into<String>,
        sandboxed: bool,
    ) -> Self {
        Self {
            schema_version: PROVENANCE_SCHEMA_VERSION,
            origin,
            source_ref: source_ref.into(),
            event: event.into(),
            sandboxed,
        }
    }

    /// Replay is never promotable. Live promotion requires a push/manual
    /// run from `main`, executed in the Phase 4 sandbox.
    pub fn promotion_error(&self) -> Option<String> {
        if self.origin != Origin::Live {
            return Some("origin=replay never promotes a live baseline".into());
        }
        if self.source_ref != "refs/heads/main" {
            return Some(format!(
                "promotion requires refs/heads/main, got {}",
                self.source_ref
            ));
        }
        if !matches!(self.event.as_str(), "push" | "workflow_dispatch") {
            return Some(format!(
                "promotion requires a push/workflow_dispatch event, got {}",
                self.event
            ));
        }
        if !self.sandboxed {
            return Some("promotion requires a sandboxed run".into());
        }
        None
    }
}

pub fn provenance_path(store: &Path, suite: &str, ref_name: &str, run_id: &str) -> PathBuf {
    store
        .join("runs")
        .join(suite)
        .join(ref_name)
        .join(format!("run-{run_id}"))
        .join("provenance.json")
}

pub fn write_provenance(
    store: &Path,
    suite: &str,
    ref_name: &str,
    run_id: &str,
    provenance: &Provenance,
) -> Result<()> {
    let path = provenance_path(store, suite, ref_name, run_id);
    let bytes = serde_json::to_vec_pretty(provenance).context("serializing provenance")?;
    std::fs::write(&path, bytes)
        .with_context(|| format!("writing provenance {}", path.display()))?;
    Ok(())
}

pub fn read_provenance(
    store: &Path,
    suite: &str,
    ref_name: &str,
    run_id: &str,
) -> Result<Option<Provenance>> {
    let path = provenance_path(store, suite, ref_name, run_id);
    match std::fs::read(&path) {
        Ok(bytes) => {
            Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                format!("parsing provenance {}", path.display())
            })?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading provenance {}", path.display())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)]
pub struct PrContext {
    pub event: String,
    pub base_repo: String,
    pub head_repo: String,
    pub is_fork: bool,
    pub allows_execution: bool,
    pub allows_secrets: bool,
    pub allows_live: bool,
}

#[allow(dead_code)]
impl PrContext {
    /// Parse the GitHub event payload. `pull_request_target` is always
    /// rejected; forks may run offline checks but never receive secrets
    /// or live provider keys.
    pub fn from_event(event: &str, payload: &Value) -> Self {
        let base_repo = payload
            .pointer("/pull_request/base/repo/full_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let head_repo = payload
            .pointer("/pull_request/head/repo/full_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let is_fork = !head_repo.is_empty() && head_repo != base_repo;
        let allows_execution = event == "pull_request" || event == "push";
        let allows_secrets = event == "push" && !is_fork;
        let allows_live = event == "push"
            && payload
                .get("ref")
                .and_then(Value::as_str)
                .is_some_and(|r| r == "refs/heads/main");
        Self {
            event: event.to_string(),
            base_repo,
            head_repo,
            is_fork,
            allows_execution,
            allows_secrets,
            allows_live,
        }
    }
}

pub struct GateArgs {
    pub suite: PathBuf,
    pub store: PathBuf,
    pub baseline_ref: String,
    pub candidate_ref: String,
    pub baseline_id: Option<String>,
    pub candidate_id: Option<String>,
    pub origin: Option<String>,
    pub promote: bool,
    pub json: bool,
}

pub fn cmd_gate(args: GateArgs) -> Result<i32> {
    let suite = Suite::load(&args.suite).context("loading suite")?;
    let store = RunStore::new(&args.store);
    let resolve = |ref_name: &str, id: Option<String>| -> Result<(String, eval_core::RunRecord)> {
        let id = match id {
            Some(id) => id,
            None => store
                .latest(&suite.suite, ref_name)?
                .context(format!("no runs for {}/{ref_name}", suite.suite))?,
        };
        let record = store.load(&suite.suite, ref_name, &id)?;
        Ok((id, record))
    };
    let (base_id, baseline) = resolve(&args.baseline_ref, args.baseline_id)?;
    let (cand_id, mut candidate) = resolve(&args.candidate_ref, args.candidate_id)?;

    let provenance = read_provenance(&args.store, &suite.suite, &args.candidate_ref, &cand_id)?;
    if let Some(origin) = &args.origin {
        let expected = Origin::parse(origin)?;
        let actual = provenance
            .as_ref()
            .context("candidate has no provenance; rerun eval run with --origin")?
            .origin;
        if actual != expected {
            bail!(
                "candidate provenance is {}, but --origin {} was requested",
                actual.as_str(),
                expected.as_str()
            );
        }
    }

    let decision: GateDecision =
        eval_graders::decide_gate(Some(&baseline), &candidate, &suite.gate);
    let pass = decision.pass;
    candidate.gate = Some(decision.clone());
    let decision_path = args
        .store
        .join("runs")
        .join(&suite.suite)
        .join(&args.candidate_ref)
        .join(format!("run-{cand_id}"))
        .join("decision.json");
    std::fs::write(&decision_path, serde_json::to_string_pretty(&decision)?)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&decision)?);
    } else {
        println!(
            "gate [{}] {}: baseline {} vs candidate {}",
            if pass { "pass" } else { "fail" },
            suite.suite,
            base_id,
            cand_id
        );
        for evidence in &decision.evidence {
            println!("  {evidence}");
        }
    }

    if args.promote {
        if !pass {
            println!("gate: refusing to promote a failing candidate");
            return Ok(1);
        }
        let provenance = provenance.context(
            "candidate has no provenance; promotion requires a main-branch sandboxed live run",
        )?;
        if let Some(error) = provenance.promotion_error() {
            println!("gate: refusing promotion: {error}");
            return Ok(1);
        }
        store.set_baseline(&BaselinePointer {
            suite: suite.suite.clone(),
            ref_name: args.candidate_ref.clone(),
            run_id: cand_id.clone(),
        })?;
        println!(
            "promoted: {} / {} -> run {cand_id}",
            suite.suite, args.candidate_ref
        );
    }

    Ok(if pass { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replay_never_promotes() {
        let provenance = Provenance::new(Origin::Replay, "refs/heads/main", "push", true);
        assert!(provenance.promotion_error().unwrap().contains("replay"));
    }

    #[test]
    fn main_sandboxed_live_promotes() {
        let provenance = Provenance::new(Origin::Live, "refs/heads/main", "push", true);
        assert!(provenance.promotion_error().is_none());
    }

    #[test]
    fn host_or_feature_branch_never_promotes() {
        let host = Provenance::new(Origin::Live, "refs/heads/main", "push", false);
        assert!(host.promotion_error().unwrap().contains("sandboxed"));
        let feature = Provenance::new(Origin::Live, "refs/heads/topic", "push", true);
        assert!(
            feature
                .promotion_error()
                .unwrap()
                .contains("refs/heads/main")
        );
    }

    #[test]
    fn pull_request_target_is_rejected() {
        let context = PrContext::from_event(
            "pull_request_target",
            &json!({"pull_request": {"head": {"repo": {"full_name": "a/b"}}, "base": {"repo": {"full_name": "a/b"}}}}),
        );
        assert!(!context.allows_execution);
        assert!(!context.allows_secrets);
        assert!(!context.allows_live);
    }

    #[test]
    fn fork_pr_gets_no_secrets_or_live() {
        let context = PrContext::from_event(
            "pull_request",
            &json!({"pull_request": {"head": {"repo": {"full_name": "fork/eval"}}, "base": {"repo": {"full_name": "owner/eval"}}}}),
        );
        assert!(context.is_fork);
        assert!(context.allows_execution);
        assert!(!context.allows_secrets);
        assert!(!context.allows_live);
    }

    #[test]
    fn main_push_can_promote() {
        let context = PrContext::from_event("push", &json!({"ref": "refs/heads/main"}));
        assert!(context.allows_live);
        assert!(context.allows_secrets);
    }

    #[test]
    fn a_regression_fails_the_gate() {
        use eval_core::{CaseRecord, Grade, RunRecord, TrialObservation, TrialStatus};

        fn case(id: &str, pass: bool) -> CaseRecord {
            let trial = TrialObservation {
                case_id: id.into(),
                trial: 0,
                status: if pass {
                    TrialStatus::Ok
                } else {
                    TrialStatus::CaseError
                },
                exit_code: Some(if pass { 0 } else { 1 }),
                wall_ms: 1,
                output: None,
                events: vec![],
                stderr_tail: None,
                error_code: None,
                error_message: None,
            };
            CaseRecord {
                case_id: id.into(),
                deterministic: true,
                trials: vec![trial],
                grades: vec![Grade {
                    grader: "exact".into(),
                    pass,
                    score: Some(if pass { 1.0 } else { 0.0 }),
                    evidence: "test".into(),
                }],
                pass,
                digest: id.into(),
            }
        }

        fn record(cases: Vec<CaseRecord>) -> RunRecord {
            RunRecord {
                schema_version: 1,
                run_id: String::new(),
                suite: "demo".into(),
                suite_version: 1,
                code_version: "test".into(),
                ref_name: "work".into(),
                started_at_ms: 1,
                request_id: None,
                target: None,
                files_root: ".".into(),
                trials_total: 1,
                cases,
                gate: None,
            }
        }

        let baseline = record(vec![case("a", true)]);
        let candidate = record(vec![case("a", false)]);
        let decision = eval_graders::decide_gate(
            Some(&baseline),
            &candidate,
            &eval_core::GatePolicy::default(),
        );
        assert!(!decision.pass);
    }
}
