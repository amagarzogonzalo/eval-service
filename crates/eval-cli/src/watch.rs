//! `eval watch`: poll GitHub read-only for `eval-request` issues,
//! debounce them, run allowed suites, and persist a restart-safe queue
//! plus heartbeat. The watcher never writes to GitHub in this phase.

use anyhow::{Context, Result};
use eval_core::{RunStore, Suite, now_ms};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STATE_SCHEMA_VERSION: u32 = 1;
const HEARTBEAT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct WatchConfig {
    pub repo: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_label")]
    pub label: String,
    #[serde(default = "default_token_env")]
    pub token_env: String,
    /// Optional token file (systemd LoadCredential or an SSM-written
    /// file). Takes precedence over `token_env`; contents are trimmed.
    #[serde(default)]
    pub token_file: Option<PathBuf>,
    #[serde(default = "default_interval")]
    pub poll_interval_seconds: u64,
    #[serde(default = "default_max_pages")]
    pub max_pages: usize,
    #[serde(default = "default_state_path")]
    pub state_path: PathBuf,
    #[serde(default)]
    pub heartbeat_path: Option<PathBuf>,
    #[serde(default)]
    pub debounce_seconds: u64,
    #[serde(default = "default_stale_seconds")]
    pub stale_run_seconds: u64,
    #[serde(default = "default_store")]
    pub store: PathBuf,
    #[serde(default = "default_ref")]
    pub ref_name: String,
    #[serde(default = "default_workers")]
    pub workers: usize,
    #[serde(default)]
    pub root: PathBuf,
    pub suites: Vec<String>,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub dashboard_out: Option<PathBuf>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Optional host hook run after a successful stored run (for example
    /// an S3 store sync). It never runs for failed or unisolated runs.
    #[serde(default)]
    pub post_run_command: Vec<String>,
    /// Docker isolation is mandatory for unattended runs. There is no
    /// host-execution fallback.
    #[serde(default)]
    pub sandbox: Option<WatchSandbox>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchSandbox {
    #[serde(default = "default_sandbox_kind")]
    pub kind: String,
    pub image: String,
    pub network: String,
    #[serde(default)]
    pub mounts: Vec<PathBuf>,
    #[serde(default = "default_sandbox_memory")]
    pub memory_mb: u64,
    #[serde(default = "default_sandbox_cpus")]
    pub cpus: f64,
    #[serde(default = "default_sandbox_pids")]
    pub pids: u64,
    #[serde(default = "default_sandbox_user")]
    pub user: String,
}

fn default_sandbox_kind() -> String {
    "docker".to_string()
}
fn default_sandbox_memory() -> u64 {
    2048
}
fn default_sandbox_cpus() -> f64 {
    2.0
}
fn default_sandbox_pids() -> u64 {
    128
}
fn default_sandbox_user() -> String {
    "65534:65534".to_string()
}

impl WatchSandbox {
    fn to_spec(&self) -> eval_runner::SandboxSpec {
        eval_runner::SandboxSpec {
            image: self.image.clone(),
            network: self.network.clone(),
            mounts: self.mounts.clone(),
            memory_mb: self.memory_mb,
            cpus: self.cpus,
            pids: self.pids,
            user: self.user.clone(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.kind != "docker" {
            anyhow::bail!("watch.sandbox.kind must be \"docker\"");
        }
        if matches!(self.network.trim(), "" | "host" | "bridge" | "default") {
            anyhow::bail!(
                "watch.sandbox.network must be a dedicated internal network, not {:?}",
                self.network
            );
        }
        if cfg!(windows) {
            anyhow::bail!(
                "docker sandbox is supported on the Linux runner (or WSL), not native Windows"
            );
        }
        self.to_spec()
            .validate()
            .map_err(|error| anyhow::anyhow!("watch.sandbox: {error}"))?;
        Ok(())
    }
}

fn default_base_url() -> String {
    "https://api.github.com".to_string()
}
fn default_label() -> String {
    "eval-request".to_string()
}
fn default_token_env() -> String {
    "EVAL_WATCH_TOKEN".to_string()
}
fn default_interval() -> u64 {
    60
}
fn default_max_pages() -> usize {
    5
}
fn default_state_path() -> PathBuf {
    PathBuf::from("eval-watch/state.json")
}
fn default_stale_seconds() -> u64 {
    7200
}
fn default_store() -> PathBuf {
    PathBuf::from("eval-runs")
}
fn default_ref() -> String {
    "watch".to_string()
}
fn default_workers() -> usize {
    1
}
fn default_max_attempts() -> u32 {
    2
}

impl WatchConfig {
    fn validate(&self) -> Result<()> {
        let parts: Vec<&str> = self.repo.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            anyhow::bail!("watch.repo must be owner/name");
        }
        if self.suites.is_empty() {
            anyhow::bail!("watch.suites must list at least one allowed suite");
        }
        if self.poll_interval_seconds == 0 {
            anyhow::bail!("watch.poll_interval_seconds must be > 0");
        }
        if self.max_pages == 0 {
            anyhow::bail!("watch.max_pages must be > 0");
        }
        let sandbox = self.sandbox.as_ref().ok_or_else(|| {
            anyhow::anyhow!("watch.sandbox is required; refusing unattended host execution")
        })?;
        sandbox.validate()?;
        Ok(())
    }

    fn resolve(&mut self, anchor: &Path) {
        self.state_path = resolve_path(anchor, &self.state_path);
        self.heartbeat_path = self
            .heartbeat_path
            .as_ref()
            .map(|path| resolve_path(anchor, path));
        self.store = resolve_path(anchor, &self.store);
        self.root = resolve_path(anchor, &self.root);
        self.workspace = self
            .workspace
            .as_ref()
            .map(|path| resolve_path(anchor, path));
        self.dashboard_out = self
            .dashboard_out
            .as_ref()
            .map(|path| resolve_path(anchor, path));
        self.token_file = self
            .token_file
            .as_ref()
            .map(|path| resolve_path(anchor, path));
    }

    fn heartbeat_path(&self) -> PathBuf {
        self.heartbeat_path.clone().unwrap_or_else(|| {
            self.state_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("heartbeat.json")
        })
    }
}

fn resolve_path(anchor: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        anchor.join(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchRequest {
    issue: u64,
    suite: String,
    first_seen_ms: u64,
    last_seen_ms: u64,
    attempts: u32,
    status: RequestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_attempt_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pass: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchState {
    schema_version: u32,
    repo: String,
    last_poll_ms: u64,
    requests: BTreeMap<String, WatchRequest>,
}

impl WatchState {
    fn new(repo: &str) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            repo: repo.to_string(),
            last_poll_ms: 0,
            requests: BTreeMap::new(),
        }
    }

    fn load(path: &Path, repo: &str) -> Result<Self> {
        let read = |candidate: &Path| -> Result<Self> {
            let text = std::fs::read_to_string(candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let state: Self = serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", candidate.display()))?;
            if state.schema_version > STATE_SCHEMA_VERSION {
                anyhow::bail!(
                    "watch state schema v{} is newer than this binary (v{})",
                    state.schema_version,
                    STATE_SCHEMA_VERSION
                );
            }
            if state.repo != repo {
                anyhow::bail!("watch state belongs to {}, not {}", state.repo, repo);
            }
            Ok(state)
        };
        match read(path) {
            Ok(state) => Ok(state),
            Err(primary) => {
                let backup = path.with_extension("bak");
                if backup.exists() {
                    eprintln!(
                        "eval watch: primary state unreadable ({primary}); trying {}",
                        backup.display()
                    );
                    return read(&backup);
                }
                Err(primary)
            }
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut state = self.clone();
        state.prune_history();
        let bytes = serde_json::to_vec_pretty(&state)?;
        atomic_write(path, &bytes)
    }

    fn prune_history(&mut self) {
        const MAX_DONE: usize = 1000;
        let done: Vec<String> = self
            .requests
            .iter()
            .filter(|(_, request)| {
                matches!(request.status, RequestStatus::Done | RequestStatus::Failed)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if done.len() <= MAX_DONE {
            return;
        }
        let mut finished: Vec<(String, u64)> = done
            .into_iter()
            .map(|id| {
                let at = self.requests[&id].finished_at_ms.unwrap_or(0);
                (id, at)
            })
            .collect();
        finished.sort_by_key(|(_, at)| *at);
        for (id, _) in finished.iter().take(finished.len() - MAX_DONE) {
            self.requests.remove(id);
        }
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    if path.exists() {
        let backup = path.with_extension("bak");
        let _ = std::fs::copy(path, &backup);
    }
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("replacing {}", path.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

struct WatchLock {
    path: PathBuf,
}

impl WatchLock {
    fn acquire(state_path: &Path) -> Result<Self> {
        let path = state_path.with_extension("lock");
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "another watcher holds {}; remove the lock only after verifying no watcher is running",
                    path.display()
                )
            })?;
        writeln!(file, "pid={}", std::process::id())?;
        Ok(Self { path })
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Deserialize)]
struct GithubIssue {
    number: u64,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestSpec {
    suite: String,
}

fn extract_request(body: &str) -> Result<Option<RequestSpec>, String> {
    let mut in_block = false;
    let mut payload = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if !in_block {
            if trimmed.eq_ignore_ascii_case("```eval-request") {
                in_block = true;
            }
            continue;
        }
        if trimmed == "```" {
            if payload.trim().is_empty() {
                return Err("empty eval-request block".to_string());
            }
            let spec: RequestSpec = serde_json::from_str(payload.trim())
                .map_err(|e| format!("bad request JSON: {e}"))?;
            if spec.suite.trim().is_empty() {
                return Err("request suite is empty".to_string());
            }
            return Ok(Some(spec));
        }
        if payload.len() > 4096 {
            return Err("eval-request block exceeds 4 KiB".to_string());
        }
        payload.push_str(line);
        payload.push('\n');
    }
    if in_block {
        return Err("unterminated eval-request block".to_string());
    }
    Ok(None)
}

fn request_id(repo: &str, issue: u64, body_hash: &str, suite: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(repo.as_bytes());
    hasher.update(b"\0");
    hasher.update(issue.to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(body_hash.as_bytes());
    hasher.update(b"\0");
    hasher.update(suite.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn body_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

async fn fetch_issues(config: &WatchConfig, token: &str) -> Result<Vec<GithubIssue>> {
    let client = reqwest::Client::builder()
        .user_agent("eval-watch/0.1")
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building GitHub HTTP client")?;
    let mut page = 1usize;
    let mut issues = Vec::new();
    while page <= config.max_pages {
        let page_text = page.to_string();
        let url = format!(
            "{}/repos/{}/issues",
            config.base_url.trim_end_matches('/'),
            config.repo
        );
        let response = client
            .get(&url)
            .query(&[
                ("state", "open"),
                ("labels", config.label.as_str()),
                ("per_page", "100"),
                ("page", page_text.as_str()),
                ("sort", "updated"),
                ("direction", "desc"),
            ])
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unknown");
            anyhow::bail!("GitHub API returned {status} for {url} (rate-limit reset: {reset})");
        }
        let next = response
            .headers()
            .get("link")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.contains("rel=\"next\""))
            .unwrap_or(false);
        let batch: Vec<GithubIssue> = response
            .json()
            .await
            .with_context(|| format!("decoding GitHub issues from {url}"))?;
        let count = batch.len();
        issues.extend(
            batch
                .into_iter()
                .filter(|issue| issue.pull_request.is_none()),
        );
        if !next || count < 100 {
            break;
        }
        page += 1;
    }
    Ok(issues)
}

struct PollSummary {
    seen: usize,
    enqueued: usize,
    cancelled: usize,
    skipped: usize,
}

fn merge_issues(
    config: &WatchConfig,
    state: &mut WatchState,
    issues: &[GithubIssue],
    now: u64,
) -> PollSummary {
    let allow: BTreeSet<&str> = config.suites.iter().map(String::as_str).collect();
    let mut current_issue: BTreeSet<u64> = BTreeSet::new();
    let mut enqueued = 0;
    let mut skipped = 0;
    for issue in issues {
        if issue.state != "open" {
            continue;
        }
        let body = issue.body.as_deref().unwrap_or("");
        let spec = match extract_request(body) {
            Ok(Some(spec)) => spec,
            Ok(None) => {
                skipped += 1;
                continue;
            }
            Err(error) => {
                eprintln!("eval watch: skipping issue #{}: {error}", issue.number);
                skipped += 1;
                continue;
            }
        };
        if !allow.contains(spec.suite.as_str()) {
            eprintln!(
                "eval watch: skipping issue #{}: suite `{}` is not allowlisted",
                issue.number, spec.suite
            );
            skipped += 1;
            continue;
        }
        let hash = body_hash(body);
        let id = request_id(&config.repo, issue.number, &hash, &spec.suite);
        current_issue.insert(issue.number);
        if let Some(existing) = state.requests.get_mut(&id) {
            existing.last_seen_ms = now;
            continue;
        }
        state.requests.insert(
            id,
            WatchRequest {
                issue: issue.number,
                suite: spec.suite,
                first_seen_ms: now,
                last_seen_ms: now,
                attempts: 0,
                status: RequestStatus::Pending,
                started_at_ms: None,
                finished_at_ms: None,
                next_attempt_ms: None,
                run_id: None,
                pass: None,
                error: None,
            },
        );
        enqueued += 1;
    }
    let mut cancelled = 0;
    state.requests.retain(|_, request| {
        if request.status != RequestStatus::Pending {
            return true;
        }
        if current_issue.contains(&request.issue) {
            return true;
        }
        cancelled += 1;
        false
    });
    PollSummary {
        seen: current_issue.len(),
        enqueued,
        cancelled,
        skipped,
    }
}

fn due_requests(state: &WatchState, now: u64, debounce_ms: u64) -> Vec<(String, WatchRequest)> {
    let mut due = Vec::new();
    for (id, request) in &state.requests {
        if request.status != RequestStatus::Pending {
            continue;
        }
        let debounce_at = request.first_seen_ms.saturating_add(debounce_ms);
        let retry_at = request.next_attempt_ms.unwrap_or(0);
        if now >= debounce_at.max(retry_at) {
            due.push((id.clone(), request.clone()));
        }
    }
    due.sort_by_key(|(_, request)| request.first_seen_ms);
    due
}

fn recover_interrupted(config: &WatchConfig, state: &mut WatchState) {
    let store = RunStore::new(&config.store);
    let now = now_ms();
    let mut recovered = 0;
    for (id, request) in state.requests.iter_mut() {
        if request.status != RequestStatus::Running {
            continue;
        }
        let suite_name = Suite::load(&config.root.join(&request.suite))
            .map(|suite| suite.suite)
            .unwrap_or_else(|_| request.suite.clone());
        if let Some(record) = store.find_request_id(&suite_name, id) {
            request.status = RequestStatus::Done;
            request.run_id = Some(record.run_id.clone());
            request.pass = Some(record.pass());
            request.finished_at_ms = Some(now);
            continue;
        }
        request.status = RequestStatus::Pending;
        request.error = Some("interrupted by watcher restart".to_string());
        request.next_attempt_ms = Some(now);
        request.started_at_ms = None;
        recovered += 1;
    }
    if recovered > 0 {
        eprintln!("eval watch: recovered {recovered} interrupted request(s)");
    }
}

async fn execute_request(
    config: &WatchConfig,
    id: &str,
    request: &WatchRequest,
) -> Result<(String, bool)> {
    let suite_path = config.root.join(&request.suite);
    let suite =
        Suite::load(&suite_path).with_context(|| format!("loading {}", suite_path.display()))?;
    let target = suite.target.clone().context("suite has no target")?;
    let mut policy = eval_runner::RunPolicy::default_for(suite.env_allowlist.clone());
    policy.workers = config.workers.max(1);
    let sandbox = config
        .sandbox
        .as_ref()
        .context("watch.sandbox is required; refusing unattended host execution")?;
    policy.sandbox = Some(sandbox.to_spec());
    let store = RunStore::new(&config.store);
    if let Some(record) = store.find_request_id(&suite.suite, id) {
        return Ok((record.run_id.clone(), record.pass()));
    }
    let (record, run_id) = crate::execute_and_store(
        &suite,
        target,
        &policy,
        &config.ref_name,
        &config.store,
        &config.root,
        false,
        Some(id),
    )
    .await?;
    Ok((run_id, record.pass()))
}

fn run_post_run(config: &WatchConfig, request: &WatchRequest, run_id: &str) -> Result<()> {
    if config.post_run_command.is_empty() {
        return Ok(());
    }
    let program = config.post_run_command[0].trim();
    if program.is_empty() {
        anyhow::bail!("post_run_command[0] is empty");
    }
    let mut command = std::process::Command::new(program);
    command.args(&config.post_run_command[1..]);
    command.current_dir(&config.root);
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "LANG",
        "AWS_DEFAULT_REGION",
        "AWS_REGION",
        "AWS_PROFILE",
    ] {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    command.env("EVAL_RUN_ID", run_id);
    command.env("EVAL_SUITE", &request.suite);
    command.env("EVAL_STORE", &config.store);
    let output = command
        .output()
        .with_context(|| format!("running post_run_command {program}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "post_run_command exited {}: {}",
            output
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(500)
                .collect::<String>()
        );
    }
    Ok(())
}

fn refresh_dashboard(config: &WatchConfig) -> Result<()> {
    let (Some(workspace), Some(out)) = (&config.workspace, &config.dashboard_out) else {
        return Ok(());
    };
    let report = crate::load_portfolio(workspace)?;
    let html = eval_graders::dashboard::render_portfolio(&report);
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::write(out.join("index.html"), html)
        .with_context(|| format!("writing {}/index.html", out.display()))?;
    Ok(())
}

#[derive(Serialize)]
struct RunningInfo {
    request_id: String,
    issue: u64,
    suite: String,
    started_at_ms: u64,
}

#[derive(Serialize)]
struct Heartbeat {
    schema_version: u32,
    repo: String,
    pid: u32,
    last_beat_ms: u64,
    last_poll_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_run_id: Option<String>,
    pending: usize,
    running: Vec<RunningInfo>,
    failed: usize,
    stale_running: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

fn write_heartbeat(config: &WatchConfig, state: &WatchState, last_error: Option<&str>) {
    let now = now_ms();
    let stale_ms = config.stale_run_seconds.saturating_mul(1000);
    let mut running = Vec::new();
    let mut stale_running = 0;
    for (id, request) in &state.requests {
        if request.status != RequestStatus::Running {
            continue;
        }
        let started = request.started_at_ms.unwrap_or(now);
        if now.saturating_sub(started) > stale_ms {
            stale_running += 1;
        }
        running.push(RunningInfo {
            request_id: id.clone(),
            issue: request.issue,
            suite: request.suite.clone(),
            started_at_ms: started,
        });
    }
    let last_run = state
        .requests
        .values()
        .filter(|request| request.status == RequestStatus::Done)
        .max_by_key(|request| request.finished_at_ms.unwrap_or(0));
    let heartbeat = Heartbeat {
        schema_version: HEARTBEAT_SCHEMA_VERSION,
        repo: config.repo.clone(),
        pid: std::process::id(),
        last_beat_ms: now,
        last_poll_ms: state.last_poll_ms,
        last_run_ms: last_run.and_then(|request| request.finished_at_ms),
        last_run_id: last_run.and_then(|request| request.run_id.clone()),
        pending: state
            .requests
            .values()
            .filter(|request| request.status == RequestStatus::Pending)
            .count(),
        running,
        failed: state
            .requests
            .values()
            .filter(|request| request.status == RequestStatus::Failed)
            .count(),
        stale_running,
        last_error: last_error.map(str::to_string),
    };
    if stale_running > 0 {
        eprintln!(
            "eval watch: ALARM {stale_running} running request(s) exceeded {}s",
            config.stale_run_seconds
        );
    }
    let path = config.heartbeat_path();
    if let Ok(bytes) = serde_json::to_vec_pretty(&heartbeat) {
        if let Err(error) = atomic_write(&path, &bytes) {
            eprintln!("eval watch: heartbeat write failed: {error}");
        }
    }
}

async fn process_due(
    config: &WatchConfig,
    state: &mut WatchState,
    last_error: &mut Option<String>,
) -> Result<()> {
    let now = now_ms();
    let debounce_ms = config.debounce_seconds.saturating_mul(1000);
    let due = due_requests(state, now, debounce_ms);
    for (id, request) in due {
        {
            let entry = state.requests.get_mut(&id).expect("due request exists");
            entry.status = RequestStatus::Running;
            entry.started_at_ms = Some(now_ms());
            entry.error = None;
        }
        state.save(&config.state_path)?;
        write_heartbeat(config, state, last_error.as_deref());
        eprintln!(
            "eval watch: running issue #{} suite {} (request {})",
            request.issue, request.suite, id
        );
        match execute_request(config, &id, &request).await {
            Ok((run_id, pass)) => {
                let entry = state.requests.get_mut(&id).expect("running request exists");
                entry.status = RequestStatus::Done;
                entry.run_id = Some(run_id.clone());
                entry.pass = Some(pass);
                entry.finished_at_ms = Some(now_ms());
                eprintln!(
                    "eval watch: done issue #{} suite {} -> run {run_id} ({})",
                    request.issue,
                    request.suite,
                    if pass { "pass" } else { "fail" }
                );
                if let Err(error) = refresh_dashboard(config) {
                    *last_error = Some(format!("dashboard refresh: {error}"));
                    eprintln!("eval watch: dashboard refresh failed: {error}");
                }
                if let Err(error) = run_post_run(config, &request, &run_id) {
                    *last_error = Some(format!("post-run hook: {error}"));
                    eprintln!("eval watch: post-run hook failed: {error}");
                }
            }
            Err(error) => {
                let entry = state.requests.get_mut(&id).expect("running request exists");
                entry.attempts = entry.attempts.saturating_add(1);
                entry.error = Some(format!("{error:#}"));
                entry.started_at_ms = None;
                if entry.attempts >= config.max_attempts {
                    entry.status = RequestStatus::Failed;
                    entry.finished_at_ms = Some(now_ms());
                    *last_error = Some(format!("request {id}: {error:#}"));
                    eprintln!("eval watch: failed issue #{}: {error:#}", request.issue);
                } else {
                    entry.status = RequestStatus::Pending;
                    let backoff = config
                        .poll_interval_seconds
                        .saturating_mul(2u64.pow(entry.attempts));
                    entry.next_attempt_ms = Some(now_ms().saturating_add(backoff * 1000));
                    eprintln!(
                        "eval watch: retrying issue #{} in {}s: {error:#}",
                        request.issue, backoff
                    );
                }
            }
        }
        state.save(&config.state_path)?;
        write_heartbeat(config, state, last_error.as_deref());
    }
    Ok(())
}

fn read_token(config: &WatchConfig) -> Result<String> {
    if let Some(path) = &config.token_file {
        let token = std::fs::read_to_string(path)
            .with_context(|| format!("reading watch token {}", path.display()))?;
        let token = token.trim();
        if token.is_empty() {
            anyhow::bail!("watch token file {} is empty", path.display());
        }
        return Ok(token.to_string());
    }
    let token = std::env::var(&config.token_env).with_context(|| {
        format!(
            "{} is not set; set a read-only token or configure watch.token_file",
            config.token_env
        )
    })?;
    if token.trim().is_empty() {
        anyhow::bail!("{} is empty", config.token_env);
    }
    Ok(token)
}

pub struct WatchArgs {
    pub config: PathBuf,
    pub once: bool,
    pub dry_run: bool,
    pub interval: Option<u64>,
    pub store: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub token_env: Option<String>,
    pub base_url: Option<String>,
    pub debounce_seconds: Option<u64>,
}

pub async fn cmd_watch(args: WatchArgs) -> Result<i32> {
    let text = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading {}", args.config.display()))?;
    let mut config: WatchConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", args.config.display()))?;
    let anchor = args
        .config
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    config.resolve(&anchor);
    if let Some(interval) = args.interval {
        config.poll_interval_seconds = interval;
    }
    if let Some(store) = args.store {
        config.store = resolve_path(&anchor, &store);
    }
    if let Some(state) = args.state {
        config.state_path = resolve_path(&anchor, &state);
    }
    if let Some(token_env) = args.token_env {
        config.token_env = token_env;
    }
    if let Some(base_url) = args.base_url {
        config.base_url = base_url;
    }
    if let Some(debounce) = args.debounce_seconds {
        config.debounce_seconds = debounce;
    }
    config.validate()?;

    let token = read_token(&config)?;

    let _lock = WatchLock::acquire(&config.state_path)?;
    let state_exists =
        config.state_path.exists() || config.state_path.with_extension("bak").exists();
    let mut state = if state_exists {
        WatchState::load(&config.state_path, &config.repo)?
    } else {
        WatchState::new(&config.repo)
    };
    recover_interrupted(&config, &mut state);
    state.save(&config.state_path)?;

    let mut last_error: Option<String> = None;
    loop {
        match fetch_issues(&config, &token).await {
            Ok(issues) => {
                if last_error.take().is_some() {
                    eprintln!("eval watch: recovered from previous poll error");
                }
                let now = now_ms();
                let summary = merge_issues(&config, &mut state, &issues, now);
                state.last_poll_ms = now;
                if summary.enqueued > 0 || summary.cancelled > 0 {
                    eprintln!(
                        "eval watch: poll #{} seen={} enqueued={} cancelled={} skipped={}",
                        issues.len(),
                        summary.seen,
                        summary.enqueued,
                        summary.cancelled,
                        summary.skipped
                    );
                }
                if args.dry_run {
                    let due =
                        due_requests(&state, now, config.debounce_seconds.saturating_mul(1000));
                    for (id, request) in &due {
                        println!(
                            "would run {} issue #{} {}",
                            id, request.issue, request.suite
                        );
                    }
                    println!("dry run: {} due request(s)", due.len());
                    write_heartbeat(&config, &state, None);
                    return Ok(0);
                }
                state.save(&config.state_path)?;
                process_due(&config, &mut state, &mut last_error).await?;
                write_heartbeat(&config, &state, last_error.as_deref());
            }
            Err(error) => {
                last_error = Some(format!("{error:#}"));
                eprintln!("eval watch: poll failed: {error:#}");
                write_heartbeat(&config, &state, last_error.as_deref());
                if args.once {
                    return Ok(2);
                }
            }
        }
        if args.once {
            return Ok(0);
        }
        tokio::time::sleep(Duration::from_secs(config.poll_interval_seconds)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WatchConfig {
        WatchConfig {
            repo: "owner/repo".to_string(),
            base_url: "https://api.github.com".to_string(),
            label: "eval-request".to_string(),
            token_env: "EVAL_WATCH_TOKEN".to_string(),
            token_file: None,
            poll_interval_seconds: 60,
            max_pages: 1,
            state_path: PathBuf::from("state.json"),
            heartbeat_path: None,
            debounce_seconds: 10,
            stale_run_seconds: 60,
            store: PathBuf::from("runs"),
            ref_name: "watch".to_string(),
            workers: 1,
            root: PathBuf::from("."),
            suites: vec!["suite.yaml".to_string()],
            workspace: None,
            dashboard_out: None,
            max_attempts: 2,
            post_run_command: Vec::new(),
            sandbox: Some(WatchSandbox {
                kind: "docker".to_string(),
                image: "eval-sandbox:24.04".to_string(),
                network: "eval-internal".to_string(),
                mounts: Vec::new(),
                memory_mb: 512,
                cpus: 1.0,
                pids: 64,
                user: "65534:65534".to_string(),
            }),
        }
    }

    fn pending(now: u64) -> WatchRequest {
        WatchRequest {
            issue: 1,
            suite: "suite.yaml".to_string(),
            first_seen_ms: now,
            last_seen_ms: now,
            attempts: 0,
            status: RequestStatus::Pending,
            started_at_ms: None,
            finished_at_ms: None,
            next_attempt_ms: None,
            run_id: None,
            pass: None,
            error: None,
        }
    }

    #[test]
    fn parses_fenced_request_and_rejects_unknown_fields() {
        let body = "hello\n```eval-request\n{\"suite\":\"suite.yaml\"}\n```\n";
        assert_eq!(extract_request(body).unwrap().unwrap().suite, "suite.yaml");
        let bad = "```eval-request\n{\"suite\":\"suite.yaml\",\"ref\":\"x\"}\n```";
        assert!(
            extract_request(bad)
                .unwrap_err()
                .contains("bad request JSON")
        );
        assert!(extract_request("no block").unwrap().is_none());
    }

    #[test]
    fn token_file_takes_precedence_over_environment() {
        let dir = std::env::temp_dir().join(format!(
            "eval-watch-token-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        std::fs::write(&path, "  file-token\n").unwrap();
        let mut config = test_config();
        config.token_file = Some(path);
        assert_eq!(read_token(&config).unwrap(), "file-token");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn watch_refuses_unisolated_configuration() {
        let mut config = test_config();
        config.sandbox = None;
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("sandbox is required"), "{error}");

        let mut host = test_config();
        host.sandbox.as_mut().unwrap().network = "host".to_string();
        let error = host.validate().unwrap_err().to_string();
        assert!(error.contains("dedicated internal network"), "{error}");
    }

    #[test]
    fn watch_policy_carries_the_sandbox_spec() {
        let config = test_config();
        let spec = config.sandbox.as_ref().unwrap().to_spec();
        assert_eq!(spec.image, "eval-sandbox:24.04");
        assert_eq!(spec.network, "eval-internal");
        assert_eq!(spec.user, "65534:65534");
        assert_eq!(spec.memory_mb, 512);
    }

    #[test]
    fn request_id_is_stable_and_body_sensitive() {
        let one = request_id("owner/repo", 1, "hash", "suite.yaml");
        assert_eq!(one, request_id("owner/repo", 1, "hash", "suite.yaml"));
        assert_ne!(one, request_id("owner/repo", 2, "hash", "suite.yaml"));
        assert_ne!(one, request_id("owner/repo", 1, "other", "suite.yaml"));
        assert_ne!(one, request_id("owner/other", 1, "hash", "suite.yaml"));
    }

    #[test]
    fn due_respects_debounce_and_retry_window() {
        let mut state = WatchState::new("owner/repo");
        state.requests.insert("a".to_string(), pending(1_000));
        assert!(due_requests(&state, 10_999, 10_000).is_empty());
        assert_eq!(due_requests(&state, 11_000, 10_000).len(), 1);
        state.requests.get_mut("a").unwrap().next_attempt_ms = Some(20_000);
        assert!(due_requests(&state, 15_000, 10_000).is_empty());
        assert_eq!(due_requests(&state, 20_000, 10_000).len(), 1);
    }

    #[test]
    fn merge_cancels_pending_when_issue_disappears() {
        let config = test_config();
        let mut state = WatchState::new("owner/repo");
        state.requests.insert("a".to_string(), pending(1_000));
        let summary = merge_issues(&config, &mut state, &[], 2_000);
        assert_eq!(summary.cancelled, 1);
        assert!(state.requests.is_empty());
    }

    #[test]
    fn heartbeat_flags_a_stale_running_request() {
        let dir = std::env::temp_dir().join(format!(
            "eval-watch-heartbeat-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = test_config();
        config.heartbeat_path = Some(dir.join("heartbeat.json"));
        config.stale_run_seconds = 60;
        let mut request = pending(1_000);
        request.status = RequestStatus::Running;
        request.started_at_ms = Some(now_ms().saturating_sub(120_000));
        let mut state = WatchState::new("owner/repo");
        state.requests.insert("a".to_string(), request);
        write_heartbeat(&config, &state, None);
        let heartbeat: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("heartbeat.json")).unwrap())
                .unwrap();
        assert_eq!(heartbeat["stale_running"], 1);
        assert_eq!(heartbeat["running"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_adopts_a_stored_run_instead_of_rerunning() {
        let dir = std::env::temp_dir().join(format!(
            "eval-watch-recover-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = test_config();
        config.store = dir.clone();
        config.state_path = dir.join("state.json");
        config.root = dir.clone();
        std::fs::write(
            dir.join("suite.yaml"),
            "suite: watch-echo\nversion: 1\n\
             target: { argv: [\"$PYTHON\", \"echo.py\"], mode: oneshot }\n\
             env_allowlist: []\n\
             cases:\n\
             \x20 - id: echo\n\
             \x20   limits: { timeout_ms: 1000, max_output_bytes: 1024 }\n\
             \x20   graders:\n\
             \x20     - kind: exit_code\n\
             \x20       expect: 0\n",
        )
        .unwrap();
        let store = RunStore::new(&dir);
        let mut record = eval_core::RunRecord {
            schema_version: eval_core::record::SCHEMA_VERSION,
            run_id: String::new(),
            suite: "watch-echo".to_string(),
            suite_version: 1,
            code_version: "test".to_string(),
            ref_name: "watch".to_string(),
            started_at_ms: 1_000,
            request_id: Some("abc".to_string()),
            target: None,
            files_root: ".".to_string(),
            trials_total: 0,
            cases: Vec::new(),
            gate: None,
        };
        let run_id = store.save(&mut record, &[]).unwrap();
        let mut request = pending(1_000);
        request.suite = "suite.yaml".to_string();
        request.status = RequestStatus::Running;
        request.started_at_ms = Some(1_000);
        let mut state = WatchState::new("owner/repo");
        state.requests.insert("abc".to_string(), request);
        recover_interrupted(&config, &mut state);
        let recovered = &state.requests["abc"];
        assert_eq!(recovered.status, RequestStatus::Done);
        assert_eq!(recovered.run_id.as_deref(), Some(run_id.as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
