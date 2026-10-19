//! `eval-runner`: async subprocess execution for `eval/v1` targets.
//!
//! Only this crate spawns processes. Sync domain/store/graders stay
//! dependency-free. Failure taxonomy (`ok / case_error / timeout /
//! crash / protocol_error`) is never merged: a handled `case_error`
//! is a graded failure; the other three are observations of target
//! misbehavior with kill-and-respawn recovery.

use eval_core::{CaseSpec, Suite, TargetMode, TargetSpec, TrialObservation, TrialStatus};
use eval_protocol as proto;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// Stderr tail cap (8 KiB). Stdout lines are capped by the protocol's
/// 8 MiB line rule inside `parse_line`.
pub const STDERR_CAP_BYTES: usize = 8 * 1024;

/// Errors that abort a whole suite (exit 2 territory). Per-case
/// observations never surface here.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("cannot spawn target {0:?}: {1}")]
    Spawn(Vec<String>, String),
    #[error("target argv is empty")]
    EmptyArgv,
    #[error("timeout rule violated: every case needs timeout_ms >= 1.5x target budget {0}ms")]
    TimeoutRule(u64),
    #[error("io failure driving target: {0}")]
    Io(String),
}

/// Execution policy: bounded worker pool, allowlisted env.
#[derive(Debug, Clone)]
pub struct RunPolicy {
    /// Worker slots. Defaults to `min(4, parallelism)`.
    pub workers: usize,
    /// Extra env vars guaranteed present (on top of the minimal base).
    pub env_allowlist: Vec<String>,
}

impl RunPolicy {
    pub fn default_for(env_allowlist: Vec<String>) -> Self {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            workers: parallelism.clamp(1, 4),
            env_allowlist,
        }
    }
}

/// Raw per-case outcome: observations without grades.
#[derive(Debug)]
pub struct ExecutedCase {
    pub case_id: String,
    pub trials: Vec<TrialObservation>,
}

/// Raw suite outcome.
#[derive(Debug)]
pub struct SuiteRun {
    pub started_ms: u64,
    pub cases: Vec<ExecutedCase>,
}

/// Resolve `$PYTHON` in argv[0] via `EVAL_PYTHON`, else probe the
/// platform's usual interpreter commands ("python", "py", ... on
/// Windows) and use the first that actually spawns. If none spawn,
/// the platform default is still returned so the spawn error names
/// the attempted program — the engine classifies it, never guesses.
pub fn expand_argv(argv: &[String]) -> Vec<String> {
    let mut out: Vec<String> = argv.to_vec();
    if out.first().is_some_and(|a| a == "$PYTHON") {
        let from_env = std::env::var("EVAL_PYTHON")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let fallback = probe_python().unwrap_or_else(|| default_python().to_string());
        out[0] = from_env.unwrap_or(fallback);
    }
    out
}

fn default_python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

fn probe_python() -> Option<String> {
    static PROBED: OnceLock<Option<String>> = OnceLock::new();
    PROBED
        .get_or_init(|| {
            let candidates: &[&str] = if cfg!(windows) {
                &["python", "py", "python3"]
            } else {
                &["python3", "python"]
            };
            for candidate in candidates {
                let status = std::process::Command::new(candidate)
                    .arg("--version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                if matches!(status, Ok(code) if code.success()) {
                    return Some((*candidate).to_string());
                }
            }
            None
        })
        .clone()
}

/// Minimal child environment: OS-standard non-secret vars plus
/// allowlisted vars. The full parent environment is never dumped or
/// stored — only these keys reach the child. The base set deliberately
/// includes home/temp/identity vars (USERPROFILE, HOME, ...): real
/// tools resolve config and cache dirs from them, and an allowlist
/// that drops them breaks legitimate targets (demonstrated by an
/// NLTK-based CLI failing to find its home dir). Secrets stay out:
/// nothing matching key/token/secret shapes is passed unless a suite
/// explicitly allowlists it — and env contents are never recorded.
pub fn child_env(allowlist: &[String]) -> HashMap<String, String> {
    const BASE_VARS: &[&str] = &[
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "SystemDrive",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "USERNAME",
        "COMPUTERNAME",
        "OS",
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "LANG",
    ];
    let mut env = HashMap::new();
    for key in BASE_VARS {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.to_string(), value);
        }
    }
    for key in allowlist {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.clone(), value);
        }
    }
    env
}

/// Execute every case (all trials) of a suite.
pub async fn run_suite(
    suite: &Suite,
    target: &TargetSpec,
    policy: &RunPolicy,
    files_root: &Path,
) -> Result<SuiteRun, RunnerError> {
    if target.argv.is_empty() {
        return Err(RunnerError::EmptyArgv);
    }
    if let Some(budget) = target.budget_ms {
        if budget > 0 {
            for case in &suite.cases {
                let need = (budget as f64) * eval_core::record::TIMEOUT_RULE_FACTOR;
                if (case.limits.timeout_ms as f64) < need {
                    return Err(RunnerError::TimeoutRule(budget));
                }
            }
        }
    }
    let started_ms = eval_core::now_ms();
    let argv = expand_argv(&target.argv);
    let env = child_env(&policy.env_allowlist);
    let cwd = target.cwd.clone().map(PathBuf::from);

    let cases = match target.mode {
        TargetMode::Oneshot => {
            run_all_oneshot(suite, &argv, &env, cwd.as_deref(), policy, files_root).await?
        }
        TargetMode::Worker => {
            run_all_worker(suite, &argv, &env, cwd.as_deref(), policy, files_root).await?
        }
    };
    Ok(SuiteRun { started_ms, cases })
}

// ---------------------------------------------------------------------------
// Terminal classification
// ---------------------------------------------------------------------------

enum Terminal {
    Result(Value),
    CaseError(Value),
    Timeout,
    Crash(Option<i32>),
    Protocol(String),
}

fn finish_trial(
    case_id: &str,
    trial: u32,
    terminal: Terminal,
    exit_code: Option<i32>,
    wall_ms: u64,
    events: Vec<Value>,
    stderr_tail: Option<String>,
) -> TrialObservation {
    let redact = eval_core::redact_json;
    let (status, output, error_code, error_message) = match terminal {
        Terminal::Result(v) => (TrialStatus::Ok, Some(redact(&v)), None, None),
        Terminal::CaseError(v) => {
            let code = v.get("code").and_then(|c| c.as_str()).map(str::to_string);
            let message = v
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string);
            (TrialStatus::CaseError, Some(redact(&v)), code, message)
        }
        Terminal::Timeout => (
            TrialStatus::Timeout,
            None,
            Some("timeout".to_string()),
            Some("deadline exceeded; worker killed and respawned".to_string()),
        ),
        Terminal::Crash(code) => (
            TrialStatus::Crash,
            None,
            Some("crash".to_string()),
            Some(match code {
                Some(c) => format!("process exited {c} before a terminal message"),
                None => "process died before a terminal message".to_string(),
            }),
        ),
        Terminal::Protocol(message) => (
            TrialStatus::ProtocolError,
            None,
            Some("protocol_error".to_string()),
            Some(message),
        ),
    };
    TrialObservation {
        case_id: case_id.to_string(),
        trial,
        status,
        exit_code,
        wall_ms,
        output,
        events: events.iter().map(redact).collect(),
        stderr_tail: stderr_tail.map(|s| eval_core::redact_string(&s)),
        error_code,
        error_message,
    }
}

// ---------------------------------------------------------------------------
// Process plumbing
// ---------------------------------------------------------------------------

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout_lines: tokio::io::Lines<BufReader<ChildStdout>>,
    stderr: Option<ChildStderr>,
}

async fn spawn_proc(
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
) -> Result<Proc, RunnerError> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(env);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| RunnerError::Spawn(argv.to_vec(), e.to_string()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| RunnerError::Io("no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RunnerError::Io("no stdout".into()))?;
    let stderr = child.stderr.take();
    Ok(Proc {
        child,
        stdin,
        stdout_lines: BufReader::new(stdout).lines(),
        stderr,
    })
}

async fn write_line(stdin: &mut ChildStdin, line: &str) -> Result<(), RunnerError> {
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|e| RunnerError::Io(e.to_string()))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| RunnerError::Io(e.to_string()))?;
    stdin
        .flush()
        .await
        .map_err(|e| RunnerError::Io(e.to_string()))
}

async fn read_stderr_capped(stderr: Option<ChildStderr>) -> Option<String> {
    let mut stderr = stderr?;
    let mut buf = Vec::new();
    {
        use tokio::io::AsyncReadExt as _;
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > STDERR_CAP_BYTES + 1024 {
                        let skip = buf.len() - STDERR_CAP_BYTES;
                        buf.drain(..skip);
                    }
                }
                Err(_) => break,
            }
        }
    }
    if buf.is_empty() {
        return None;
    }
    let start = buf.len().saturating_sub(STDERR_CAP_BYTES);
    Some(String::from_utf8_lossy(&buf[start..]).into_owned())
}

async fn kill_and_wait(proc: &mut Proc) -> Option<i32> {
    let _ = proc.child.start_kill();
    proc.child.wait().await.ok().and_then(|s| s.code())
}

fn proto_error(message: String) -> Terminal {
    Terminal::Protocol(message)
}

/// Read lines until the terminal message for `case_id`, enforcing the
/// output budget. Returns the terminal plus all event values.
async fn read_until_terminal(
    proc: &mut Proc,
    case_id: &str,
    max_output_bytes: u64,
) -> (Terminal, Vec<Value>, u64) {
    let mut events = Vec::new();
    let mut bytes: u64 = 0;
    let mut saw_ready = false;
    loop {
        match proc.stdout_lines.next_line().await {
            Ok(Some(line)) => {
                bytes += line.len() as u64 + 1;
                if bytes > max_output_bytes {
                    return (
                        proto_error("output budget exceeded".to_string()),
                        events,
                        bytes,
                    );
                }
                // The handshake reply has no `type`; accept it once.
                if line.contains("\"ready\"") && !saw_ready {
                    match proto::parse_line(&line) {
                        Ok(proto::Inbound::Ready(_)) => {
                            saw_ready = true;
                            continue;
                        }
                        Ok(_) => {}
                        Err(e) => return (proto_error(e.to_string()), events, bytes),
                    }
                }
                match proto::parse_line(&line) {
                    Ok(proto::Inbound::Ready(_)) => {
                        return (
                            proto_error("unexpected ready after handshake".to_string()),
                            events,
                            bytes,
                        );
                    }
                    Ok(proto::Inbound::Event(e)) => {
                        if e.case_id != case_id {
                            return (
                                proto_error(format!(
                                    "cross-case line for `{}` during `{case_id}`",
                                    e.case_id
                                )),
                                events,
                                bytes,
                            );
                        }
                        let raw: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        events.push(raw);
                    }
                    Ok(proto::Inbound::Result(_)) => {
                        let raw: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        if raw.get("case_id").and_then(|c| c.as_str()) != Some(case_id) {
                            return (
                                proto_error("cross-case terminal message".to_string()),
                                events,
                                bytes,
                            );
                        }
                        return (Terminal::Result(raw), events, bytes);
                    }
                    Ok(proto::Inbound::CaseError(_)) => {
                        let raw: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                        if raw.get("case_id").and_then(|c| c.as_str()) != Some(case_id) {
                            return (
                                proto_error("cross-case terminal message".to_string()),
                                events,
                                bytes,
                            );
                        }
                        return (Terminal::CaseError(raw), events, bytes);
                    }
                    Err(e) => return (proto_error(e.to_string()), events, bytes),
                }
            }
            Ok(None) => {
                // EOF before a terminal message: crash vs clean early close.
                return match proc.child.wait().await {
                    Ok(status) if status.success() => (
                        proto_error("target closed stdout without a terminal message".to_string()),
                        events,
                        bytes,
                    ),
                    Ok(status) => (Terminal::Crash(status.code()), events, bytes),
                    Err(_) => (Terminal::Crash(None), events, bytes),
                };
            }
            Err(e) => {
                return (
                    proto_error(format!("stdout read failed: {e}")),
                    events,
                    bytes,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Oneshot execution
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_case_oneshot(
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    mode: TargetMode,
    case: &CaseSpec,
    trial: u32,
) -> TrialObservation {
    let begun = Instant::now();
    let case_line = proto_case_line(case);
    let mut proc = match spawn_proc(argv, env, cwd).await {
        Ok(proc) => proc,
        Err(e) => {
            return finish_trial(
                &case.id,
                trial,
                proto_error(format!("spawn failed: {e}")),
                None,
                begun.elapsed().as_millis() as u64,
                Vec::new(),
                None,
            );
        }
    };
    let handshake = proto::encode_handshake(match mode {
        TargetMode::Worker => proto::Mode::Worker,
        TargetMode::Oneshot => proto::Mode::Oneshot,
    });
    let outcome = async {
        write_line(&mut proc.stdin, &handshake).await?;
        write_line(&mut proc.stdin, &case_line).await?;
        proc.stdin
            .shutdown()
            .await
            .map_err(|e| RunnerError::Io(e.to_string()))?;
        Ok::<(), RunnerError>(())
    }
    .await;
    if let Err(e) = outcome {
        let code = kill_and_wait(&mut proc).await;
        let stderr = read_stderr_capped(proc.stderr.take()).await;
        return finish_trial(
            &case.id,
            trial,
            proto_error(e.to_string()),
            code,
            begun.elapsed().as_millis() as u64,
            Vec::new(),
            stderr,
        );
    }
    // Reattach a fresh stdin handle holder: stdin was moved; keep borrow simple.
    let deadline = Duration::from_millis(case.limits.timeout_ms);
    let (terminal, events, _) = tokio::time::timeout(
        deadline,
        read_until_terminal(&mut proc, &case.id, case.limits.max_output_bytes),
    )
    .await
    .unwrap_or((Terminal::Timeout, Vec::new(), 0));
    let wall_ms = begun.elapsed().as_millis() as u64;
    let (exit_code, stderr) = match &terminal {
        Terminal::Timeout | Terminal::Protocol(_) => {
            let code = kill_and_wait(&mut proc).await;
            (code, read_stderr_capped(proc.stderr.take()).await)
        }
        _ => {
            // Bounded wait: a target that answered but never exits still
            // gets reaped instead of stalling the suite.
            let code = match tokio::time::timeout(Duration::from_secs(5), proc.child.wait()).await {
                Ok(Ok(status)) => status.code(),
                _ => kill_and_wait(&mut proc).await,
            };
            (code, read_stderr_capped(proc.stderr.take()).await)
        }
    };
    finish_trial(
        &case.id, trial, terminal, exit_code, wall_ms, events, stderr,
    )
}

fn proto_case_line(case: &CaseSpec) -> String {
    let mut env = HashMap::new();
    env.insert("OFFLINE".to_string(), "1".to_string());
    let msg = serde_json::json!({
        "type": "case",
        "case_id": case.id,
        "input": {
            "argv": case.input.argv,
            "stdin_text": case.input.stdin_text,
            "params": case.input.params,
        },
        "limits": {
            "timeout_ms": case.limits.timeout_ms,
            "max_output_bytes": case.limits.max_output_bytes,
        },
        "env": env,
    });
    serde_json::to_string(&msg).expect("case line serializes")
}

async fn run_all_oneshot(
    suite: &Suite,
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    policy: &RunPolicy,
    _files_root: &Path,
) -> Result<Vec<ExecutedCase>, RunnerError> {
    let mode = suite
        .target
        .as_ref()
        .map(|t| t.mode)
        .unwrap_or(TargetMode::Oneshot);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(policy.workers.max(1)));
    let mut handles = Vec::new();
    for case in suite.cases.clone() {
        for trial in 0..case.trials {
            let sem = sem.clone();
            let argv = argv.to_vec();
            let env = env.clone();
            let cwd = cwd.map(PathBuf::from);
            let owned = case.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore");
                run_case_oneshot(&argv, &env, cwd.as_deref(), mode, &owned, trial).await
            }));
        }
    }
    let mut by_case: HashMap<String, Vec<TrialObservation>> = HashMap::new();
    for handle in handles {
        let trial = handle.await.map_err(|e| RunnerError::Io(e.to_string()))?;
        by_case
            .entry(trial.case_id.clone())
            .or_default()
            .push(trial);
    }
    Ok(suite
        .cases
        .iter()
        .map(|c| {
            let mut trials = by_case.remove(&c.id).unwrap_or_default();
            trials.sort_by_key(|t| t.trial);
            ExecutedCase {
                case_id: c.id.clone(),
                trials,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Worker-pool execution
// ---------------------------------------------------------------------------

struct Job {
    case: CaseSpec,
    trial: u32,
}

async fn run_all_worker(
    suite: &Suite,
    argv: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&Path>,
    policy: &RunPolicy,
    _files_root: &Path,
) -> Result<Vec<ExecutedCase>, RunnerError> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Job>(suite.cases.len() * 2 + 8);
    for case in &suite.cases {
        for trial in 0..case.trials {
            tx.send(Job {
                case: case.clone(),
                trial,
            })
            .await
            .map_err(|e| RunnerError::Io(e.to_string()))?;
        }
    }
    drop(tx);

    let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<TrialObservation>(1024);
    let shared_rx = std::sync::Arc::new(tokio::sync::Mutex::new(rx));
    let workers = policy.workers.max(1);
    let mut handles = Vec::new();
    for _ in 0..workers {
        handles.push(tokio::spawn(worker_loop(
            shared_rx.clone(),
            res_tx.clone(),
            argv.to_vec(),
            env.clone(),
            cwd.map(PathBuf::from),
        )));
    }
    drop(res_tx);
    for handle in handles {
        handle.await.map_err(|e| RunnerError::Io(e.to_string()))?;
    }

    let mut by_case: HashMap<String, Vec<TrialObservation>> = HashMap::new();
    while let Some(trial) = res_rx.recv().await {
        by_case
            .entry(trial.case_id.clone())
            .or_default()
            .push(trial);
    }
    Ok(suite
        .cases
        .iter()
        .map(|c| {
            let mut trials = by_case.remove(&c.id).unwrap_or_default();
            trials.sort_by_key(|t| t.trial);
            ExecutedCase {
                case_id: c.id.clone(),
                trials,
            }
        })
        .collect())
}

/// One pool worker: one persistent process, many cases. Any failed case
/// (timeout / crash / protocol violation) kills and respawns the child
/// before the next case, so failures never contaminate later cases.
async fn worker_loop(
    rx: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Job>>>,
    res_tx: tokio::sync::mpsc::Sender<TrialObservation>,
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
) {
    let mut proc = match spawn_proc(&argv, &env, cwd.as_deref()).await {
        Ok(proc) => proc,
        Err(_) => return,
    };
    if handshake(&mut proc).await.is_err() {
        return;
    }
    loop {
        let job = rx.lock().await.recv().await;
        let Some(job) = job else { break };
        let begun = Instant::now();
        let outcome = run_case_on_worker(&mut proc, &job).await;
        let wall_ms = begun.elapsed().as_millis() as u64;
        let needs_respawn = matches!(
            outcome.0,
            Terminal::Timeout | Terminal::Crash(_) | Terminal::Protocol(_)
        );
        let (terminal, events, exit_code, stderr) = outcome;
        if res_tx
            .send(finish_trial(
                &job.case.id,
                job.trial,
                terminal,
                exit_code,
                wall_ms,
                events,
                stderr,
            ))
            .await
            .is_err()
        {
            break;
        }
        if needs_respawn {
            kill_and_wait(&mut proc).await;
            match spawn_proc(&argv, &env, cwd.as_deref()).await {
                Ok(fresh) => {
                    proc = fresh;
                    if handshake(&mut proc).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let _ = write_line(&mut proc.stdin, &proto::encode_shutdown()).await;
    kill_and_wait(&mut proc).await;
}

/// Worker handshake: expect exactly one ready reply.
async fn handshake(proc: &mut Proc) -> Result<(), RunnerError> {
    write_line(
        &mut proc.stdin,
        &proto::encode_handshake(proto::Mode::Worker),
    )
    .await?;
    match tokio::time::timeout(Duration::from_secs(10), proc.stdout_lines.next_line()).await {
        Ok(Ok(Some(line))) => match proto::parse_line(&line) {
            Ok(proto::Inbound::Ready(_)) => Ok(()),
            Ok(_) => Err(RunnerError::Io("expected ready first".to_string())),
            Err(e) => Err(RunnerError::Io(e.to_string())),
        },
        _ => Err(RunnerError::Io("no handshake reply".to_string())),
    }
}

/// Run one case on a live worker process (caller owns respawn).
async fn run_case_on_worker(
    proc: &mut Proc,
    job: &Job,
) -> (Terminal, Vec<Value>, Option<i32>, Option<String>) {
    if write_line(&mut proc.stdin, &proto_case_line(&job.case))
        .await
        .is_err()
    {
        return (
            proto_error("worker stdin write failed".to_string()),
            Vec::new(),
            kill_and_wait(proc).await,
            None,
        );
    }
    let deadline = Duration::from_millis(job.case.limits.timeout_ms);
    let (terminal, events, _) = tokio::time::timeout(
        deadline,
        read_until_terminal(proc, &job.case.id, job.case.limits.max_output_bytes),
    )
    .await
    .unwrap_or((Terminal::Timeout, Vec::new(), 0));
    let (exit_code, stderr) = match &terminal {
        Terminal::Result(_) | Terminal::CaseError(_) => {
            // Healthy path: keep the worker alive; do not reap it.
            (None, None)
        }
        _ => {
            let code = kill_and_wait(proc).await;
            let tail = read_stderr_capped(proc.stderr.take()).await;
            (code, tail)
        }
    };
    (terminal, events, exit_code, stderr)
}
