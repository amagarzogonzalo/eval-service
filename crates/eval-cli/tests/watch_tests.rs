//! `eval watch` integration: a local GitHub stub, one issue, a real
//! suite run, a restart, and no duplicate run.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

fn eval_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_eval"))
}

fn tmpdir(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("eval-watch-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn shim_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/python_shim/main.py")
        .canonicalize()
        .unwrap()
}

fn python_launcher() -> String {
    if cfg!(windows) {
        "py".to_string()
    } else {
        "python3".to_string()
    }
}

fn write_suite(dir: &Path) -> PathBuf {
    let path = dir.join("suite.yaml");
    let shim = dir.join("shim.py");
    std::fs::copy(shim_path(), &shim).unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let text = "suite: watch-echo\n\
         version: 1\n\
         target:\n\
         \x20 argv: [\"python3\", \"shim.py\"]\n\
         \x20 mode: oneshot\n\
         env_allowlist: []\n\
         gate:\n\
         \x20 min_pass_rate: 1.0\n\
         cases:\n\
         \x20 - id: echo\n\
         \x20   input:\n\
         \x20     params: { behavior: echo, text: ok }\n\
         \x20   limits: { timeout_ms: 15000, max_output_bytes: 65536 }\n\
         \x20   trials: 1\n\
         \x20   deterministic: true\n\
         \x20   graders:\n\
         \x20     - kind: exact\n\
         \x20       field: output.text\n\
         \x20       value: ok\n";
    std::fs::write(&path, text).unwrap();
    path
}

struct SandboxNet {
    name: String,
}

impl Drop for SandboxNet {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["network", "rm", &self.name])
            .output();
    }
}

fn sandbox_ready() -> Option<SandboxNet> {
    if cfg!(windows) || std::env::var("EVAL_DOCKER").ok().as_deref() != Some("1") {
        eprintln!("SKIP: set EVAL_DOCKER=1 on Linux (and build eval-sandbox:24.04)");
        return None;
    }
    let image = Command::new("docker")
        .args(["image", "inspect", "eval-sandbox:24.04"])
        .output();
    match image {
        Ok(output) if output.status.success() => {}
        _ => {
            eprintln!("SKIP: eval-sandbox:24.04 missing; run `make sandbox-image`");
            return None;
        }
    }
    let name = format!(
        "eval-watch-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let created = Command::new("docker")
        .args(["network", "create", "--internal", &name])
        .output();
    match created {
        Ok(output) if output.status.success() => Some(SandboxNet { name }),
        _ => {
            eprintln!("SKIP: could not create an internal Docker network");
            None
        }
    }
}

fn write_config(
    dir: &Path,
    port: u16,
    network: &str,
    debounce: u64,
    suites: &str,
    post_run: Option<&str>,
) {
    let post_run = post_run
        .map(|value| format!("post_run_command = {value}\n"))
        .unwrap_or_default();
    let config = format!(
        "repo = \"owner/repo\"\n\
         base_url = \"http://127.0.0.1:{port}\"\n\
         token_env = \"EVAL_WATCH_TOKEN\"\n\
         label = \"eval-request\"\n\
         poll_interval_seconds = 60\n\
         debounce_seconds = {debounce}\n\
         state_path = \"state.json\"\n\
         store = \"runs\"\n\
         root = \".\"\n\
         suites = {suites}\n\
         ref_name = \"watch\"\n\
         workers = 1\n\
         {post_run}\
         [sandbox]\n\
         kind = \"docker\"\n\
         image = \"eval-sandbox:24.04\"\n\
         network = \"{network}\"\n\
         memory_mb = 512\n\
         cpus = 1.0\n\
         pids = 64\n\
         user = \"65534:65534\"\n"
    );
    std::fs::write(dir.join("eval-watch.toml"), config).unwrap();
}

fn spawn_stub_issues(connections: usize) -> (u16, std::sync::mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let body = concat!(
                "[{\"number\":1,\"state\":\"open\",\"body\":",
                "\"```eval-request\\n{\\\"suite\\\":\\\"suite.yaml\\\"}\\n```\"",
                "}]"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        }
        let _ = tx.send(());
    });
    (port, rx)
}

fn run_watch(dir: &Path) -> std::process::Output {
    Command::new(eval_bin())
        .args([
            "watch",
            "--config",
            dir.join("eval-watch.toml").to_str().unwrap(),
            "--once",
        ])
        .current_dir(dir)
        .env("EVAL_WATCH_TOKEN", "test-token")
        .env("EVAL_PYTHON", python_launcher())
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .output()
        .unwrap()
}

fn count_run_dirs(store: &Path, suite: &str) -> usize {
    let refs = match std::fs::read_dir(store.join("runs").join(suite)) {
        Ok(refs) => refs,
        Err(_) => return 0,
    };
    refs.flatten()
        .map(|ref_entry| {
            std::fs::read_dir(ref_entry.path())
                .map(|runs| runs.flatten().count())
                .unwrap_or(0)
        })
        .sum()
}

#[test]
fn watch_once_runs_and_restart_does_not_duplicate() {
    let Some(net) = sandbox_ready() else {
        return;
    };
    let dir = tmpdir("once");
    write_suite(&dir);
    let (port, done) = spawn_stub_issues(2);
    write_config(
        &dir,
        port,
        &net.name,
        0,
        "[\"suite.yaml\"]",
        Some("[\"sh\", \"-c\", \"echo hook > post-run.txt\"]"),
    );

    let first = run_watch(&dir);
    assert!(
        first.status.success(),
        "first watch failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let state_text = std::fs::read_to_string(dir.join("state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_text).unwrap();
    let requests = state["requests"].as_object().unwrap();
    assert_eq!(requests.len(), 1);
    let (id, request) = requests.iter().next().unwrap();
    assert_eq!(request["status"], "done");
    assert!(request["run_id"].is_string());
    assert_eq!(count_run_dirs(&dir.join("runs"), "watch-echo"), 1);
    let store = eval_core::RunStore::new(dir.join("runs"));
    assert!(store.find_request_id("watch-echo", id).is_some());
    let heartbeat = std::fs::read_to_string(dir.join("heartbeat.json")).unwrap();
    let heartbeat: serde_json::Value = serde_json::from_str(&heartbeat).unwrap();
    assert_eq!(heartbeat["pending"], 0);
    assert!(heartbeat["last_run_id"].is_string());
    assert!(heartbeat["stale_running"].as_u64().unwrap() == 0);
    assert_eq!(
        std::fs::read_to_string(dir.join("post-run.txt"))
            .unwrap()
            .trim(),
        "hook"
    );

    let second = run_watch(&dir);
    assert!(
        second.status.success(),
        "second watch failed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let state_text = std::fs::read_to_string(dir.join("state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state_text).unwrap();
    assert_eq!(state["requests"].as_object().unwrap().len(), 1);
    assert_eq!(count_run_dirs(&dir.join("runs"), "watch-echo"), 1);
    let _ = done.recv();
}

#[test]
fn watch_debounces_then_runs_after_restart() {
    let Some(net) = sandbox_ready() else {
        return;
    };
    let dir = tmpdir("debounce");
    write_suite(&dir);
    let (port, done) = spawn_stub_issues(2);
    write_config(&dir, port, &net.name, 1, "[\"suite.yaml\"]", None);

    let first = run_watch(&dir);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["requests"].as_object().unwrap().len(), 1);
    let (_, request) = state["requests"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .unwrap();
    assert_eq!(request["status"], "pending");
    assert_eq!(count_run_dirs(&dir.join("runs"), "watch-echo"), 0);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = run_watch(&dir);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap();
    let (_, request) = state["requests"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .unwrap();
    assert_eq!(request["status"], "done");
    assert_eq!(count_run_dirs(&dir.join("runs"), "watch-echo"), 1);
    let _ = done.recv();
}

#[test]
fn watch_rejects_non_allowlisted_suite_without_running() {
    if cfg!(windows) {
        eprintln!("SKIP: docker sandbox is Linux-only");
        return;
    }
    let dir = tmpdir("reject");
    write_suite(&dir);
    let (port, _done) = spawn_stub_issues(1);
    write_config(&dir, port, "eval-internal", 0, "[\"other.yaml\"]", None);
    let output = run_watch(&dir);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["requests"].as_object().unwrap().len(), 0);
    assert_eq!(count_run_dirs(&dir.join("runs"), "watch-echo"), 0);
    assert!(String::from_utf8_lossy(&output.stderr).contains("not allowlisted"));
}

#[test]
fn watch_refuses_to_start_without_a_sandbox() {
    let dir = tmpdir("unisolated");
    std::fs::write(
        dir.join("eval-watch.toml"),
        "repo = \"owner/repo\"\n\
         token_env = \"EVAL_WATCH_TOKEN\"\n\
         state_path = \"state.json\"\n\
         store = \"runs\"\n\
         root = \".\"\n\
         suites = [\"suite.yaml\"]\n",
    )
    .unwrap();
    let output = run_watch(&dir);
    assert_eq!(output.status.code(), Some(2), "{:?}", output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sandbox is required"), "{stderr}");
    assert!(
        !dir.join("state.json").exists(),
        "must refuse before state creation"
    );
}
