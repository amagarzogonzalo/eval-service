//! Docker sandbox policy tests.
//!
//! The command-shape tests always run. The adversarial container test is
//! gated by `EVAL_DOCKER=1` (and a built `eval-sandbox:24.04` image), so
//! the normal offline workspace suite stays Docker-free.

use eval_runner::{SandboxSpec, docker_argv};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const IMAGE: &str = "eval-sandbox:24.04";

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("eval-sandbox-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spec(mount: &Path) -> SandboxSpec {
    SandboxSpec {
        image: IMAGE.into(),
        network: "eval-internal".into(),
        mounts: vec![mount.to_path_buf()],
        memory_mb: 2048,
        cpus: 2.0,
        pids: 128,
        user: "65534:65534".into(),
    }
}

#[test]
fn sandbox_command_has_isolation_flags() {
    let dir = TestDir::new();
    let mount = dir.0.join("share");
    std::fs::create_dir_all(&mount).unwrap();
    let mut env = HashMap::new();
    env.insert("EVAL_DIGEST_API_KEY".into(), "dummy".into());
    let argv = docker_argv(
        &spec(&mount),
        &["python3".into(), "-V".into()],
        &env,
        None,
        &dir.0,
        "eval-sbx-test",
    )
    .expect("docker argv");
    let joined = argv.join(" ");

    for flag in [
        "--read-only",
        "--network eval-internal",
        "--dns 127.0.0.1",
        "--cap-drop ALL",
        "--security-opt no-new-privileges",
        "--pids-limit 128",
        "--memory 2048m",
        "--cpus 2",
        "--user 65534:65534",
        "--tmpfs /tmp:rw,noexec,nosuid,size=256m",
        "--workdir",
        "-e EVAL_DIGEST_API_KEY=dummy",
        "--volume",
        ":ro",
    ] {
        assert!(joined.contains(flag), "missing {flag:?} in {joined}");
    }
    assert!(
        !joined.contains("docker.sock"),
        "socket must not be mounted"
    );
    assert!(!joined.contains("--privileged"), "never privileged");
    assert!(!joined.contains("--network host"), "never host network");
    assert!(
        !joined.contains("OPENROUTER_API_KEY"),
        "provider key must not enter the target env"
    );
}

#[test]
fn sandbox_rejects_missing_mounts() {
    let dir = TestDir::new();
    let missing = dir.0.join("missing");
    let argv = docker_argv(
        &spec(&missing),
        &["python3".into()],
        &HashMap::new(),
        None,
        &dir.0,
        "eval-sbx-test",
    );
    assert!(argv.is_err(), "missing mounts fail closed");
}

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("spawn docker")
}

fn docker_ok(args: &[&str]) -> bool {
    docker(args).status.success()
}

struct ContainerGuard {
    network: String,
    proxy: String,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.proxy]);
        let _ = docker(&["network", "rm", &self.network]);
    }
}

#[test]
fn docker_sandbox_blocks_egress_dns_and_metadata() {
    if std::env::var("EVAL_DOCKER").ok().as_deref() != Some("1") {
        eprintln!("SKIP: set EVAL_DOCKER=1 (and build eval-sandbox:24.04)");
        return;
    }
    if !docker_ok(&["image", "inspect", IMAGE]) {
        eprintln!("SKIP: image {IMAGE} missing; run `make sandbox-image`");
        return;
    }

    let suffix = std::process::id();
    let network = format!("eval-sbx-test-{suffix}");
    let proxy = format!("eval-sbx-proxy-{suffix}");
    if !docker_ok(&[
        "network",
        "create",
        "--internal",
        "--subnet",
        "172.31.250.0/24",
        &network,
    ]) {
        eprintln!("SKIP: cannot create test network (existing subnet?)");
        return;
    }
    let _guard = ContainerGuard {
        network: network.clone(),
        proxy: proxy.clone(),
    };

    let proxy_run = docker(&[
        "run",
        "-d",
        "--rm",
        "--name",
        &proxy,
        "--network",
        &network,
        "--ip",
        "172.31.250.2",
        "--user",
        "65534:65534",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        IMAGE,
        "python3",
        "-m",
        "http.server",
        "8899",
    ]);
    assert!(
        proxy_run.status.success(),
        "proxy container: {}",
        String::from_utf8_lossy(&proxy_run.stderr)
    );
    std::thread::sleep(std::time::Duration::from_millis(500));

    let script = r#"
import json, os, socket, urllib.request
out = {}
try:
    out["proxy"] = urllib.request.urlopen("http://172.31.250.2:8899/", timeout=2).status
except Exception as error:
    out["proxy"] = type(error).__name__
def probe(host):
    try:
        sock = socket.create_connection((host, 80), timeout=2)
        sock.close()
        return "open"
    except Exception as error:
        return type(error).__name__
out["internet"] = probe("1.1.1.1")
out["metadata"] = probe("169.254.169.254")
try:
    socket.getaddrinfo("example.com", 80)
    out["dns"] = "resolved"
except Exception as error:
    out["dns"] = type(error).__name__
out["docker_socket"] = os.path.exists("/var/run/docker.sock")
out["key_in_env"] = any("OPENROUTER" in key for key in os.environ)
try:
    proc1 = open("/proc/1/environ", "rb").read().decode("utf-8", "ignore")
except Exception as error:
    proc1 = type(error).__name__
out["key_in_proc1"] = "OPENROUTER_API_KEY" in proc1
print(json.dumps(out))
"#;
    let target = docker(&[
        "run",
        "--rm",
        "--network",
        &network,
        "--dns",
        "127.0.0.1",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        "64",
        "--memory",
        "512m",
        "--cpus",
        "1",
        "--user",
        "65534:65534",
        IMAGE,
        "python3",
        "-c",
        script,
    ]);
    assert!(
        target.status.success(),
        "target container: {}",
        String::from_utf8_lossy(&target.stderr)
    );
    let stdout = String::from_utf8_lossy(&target.stdout);
    let line = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .expect("sandbox probe JSON");
    let parsed: serde_json::Value = serde_json::from_str(line).expect("valid probe JSON");

    assert_eq!(parsed["proxy"], 200, "internal proxy must be reachable");
    assert_ne!(parsed["internet"], "open", "internet must be blocked");
    assert_ne!(parsed["metadata"], "open", "metadata must be blocked");
    assert_ne!(parsed["dns"], "resolved", "external DNS must fail");
    assert_eq!(parsed["docker_socket"], false);
    assert_eq!(parsed["key_in_env"], false);
    assert_eq!(parsed["key_in_proc1"], false);
}
