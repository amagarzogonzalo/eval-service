"""Cross-platform runner for the black-box research service E2E.

Local mode starts the target FastAPI app with uvicorn, starts a free-model
eval-proxy, runs the service suite, and tears both down. Remote mode calls the
deployed Render service. Tokens are read from the environment or env files and
are never printed.

Usage:
  python integrations/research_agents/run_live_service.py --mode local
  python integrations/research_agents/run_live_service.py --mode remote
"""

from __future__ import annotations

import argparse
import os
import platform
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

from key_resolver import parse_env_file, parse_opencode

ROOT = Path(__file__).resolve().parents[2]
SUITE = ROOT / "integrations" / "research_agents" / "live_service.yaml"
SHIM = ROOT / "integrations" / "research_agents" / "reasoning_shim.py"
DEFAULT_FREE_MODEL = "nvidia/nemotron-3-ultra-550b-a55b:free"
DEFAULT_REMOTE = "https://research-orchestration-agents.onrender.com"


def log(message: str) -> None:
    print(f"research-service: {message}", file=sys.stderr, flush=True)


def free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def wait_http(url: str, proc: subprocess.Popen, timeout_s: float) -> None:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"process exited with {proc.returncode}: {url}")
        try:
            urllib.request.urlopen(url, timeout=1)
            return
        except OSError:
            time.sleep(0.5)
    raise RuntimeError(f"timed out waiting for {url}")


def resolve_key(name: str, files: list[Path], provider: str = "") -> str:
    value = os.environ.get(name, "").strip()
    if value:
        return value
    for path in files:
        value = parse_env_file(path, name)
        if value:
            return value
    if provider:
        value = parse_opencode(provider)
        if value:
            return value
    return ""


def default_eval_bin() -> Path:
    for candidate in (
        ROOT / "target" / "release" / "eval.exe",
        ROOT / "target" / "debug" / "eval.exe",
        ROOT / "target" / "release" / "eval",
        ROOT / "target" / "debug" / "eval",
        Path("/srv/eval-service/target/release/eval"),
    ):
        if candidate.exists():
            return candidate
    return ROOT / "target" / "debug" / "eval"


def default_store() -> Path:
    if platform.system() == "Windows":
        return ROOT / "eval-runs"
    return Path("/var/lib/eval-runs")


def acquire_lock(path: Path) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and time.time() - path.stat().st_mtime > 4 * 3600:
        path.unlink(missing_ok=True)
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError as exc:
        raise RuntimeError(f"another run holds {path}") from exc
    os.write(fd, str(os.getpid()).encode())
    return fd


def run_suite(
    env: dict[str, str],
    eval_bin: Path,
    ref: str,
    store: Path,
    *,
    lane: str,
    free_only: bool,
) -> int:
    command = [
        str(eval_bin),
        "run",
        str(SUITE),
        "--ref",
        ref,
        "--origin",
        "live",
        "--workers",
        "1",
        "--store",
        str(store),
        "--lane",
        lane,
        "--target-version",
        os.environ.get("EVAL_TARGET_VERSION", "research-orchestration-agents"),
        "--json",
    ]
    if free_only:
        command.insert(-1, "--free-only")
    log(f"running {SUITE.name} ({ref})")
    return subprocess.run(command, cwd=ROOT, env=env, check=False).returncode


def run_probe(env: dict[str, str], eval_bin: Path) -> int:
    command = [str(eval_bin), "check", str(SUITE), "--probe"]
    log("health/readiness probe")
    return subprocess.run(command, cwd=ROOT, env=env, check=False).returncode


def remote_mode(args: argparse.Namespace, eval_bin: Path, store: Path, env_file: Path) -> int:
    base_url = args.service_url or os.environ.get("EVAL_RESEARCH_SERVICE_URL", DEFAULT_REMOTE)
    token = resolve_key("EVAL_RESEARCH_SERVICE_TOKEN", [env_file]) or resolve_key(
        "RENDER_API_KEY", [env_file]
    )
    env = os.environ.copy()
    env["EVAL_RESEARCH_SERVICE_URL"] = base_url
    env["EVAL_RESEARCH_SERVICE_LANE"] = "remote"
    env["EVAL_RESEARCH_SERVICE_TOKEN"] = token
    env["EVAL_RESEARCH_REQUIRE_FREE_MODELS"] = "0"
    env["EVAL_RESEARCH_ARTIFACT_OUT"] = str(store / "research-service-artifacts")
    env["EVAL_PYTHON"] = sys.executable
    if args.otel_endpoint:
        env["EVAL_OTEL_ENDPOINT"] = args.otel_endpoint
    if run_probe(env, eval_bin) != 0:
        log("service health/readiness probe failed")
        return 2
    if not token:
        log("full remote run blocked: set EVAL_RESEARCH_SERVICE_TOKEN or RENDER_API_KEY")
        return 3
    return run_suite(
        env,
        eval_bin,
        args.ref or "research-service-remote",
        store,
        lane="remote-service",
        free_only=False,
    )


def local_mode(args: argparse.Namespace, eval_bin: Path, store: Path, env_file: Path) -> int:
    target = Path(
        args.target_repo or os.environ.get("EVAL_RESEARCH_REPO", "")
    ).expanduser()
    if not target.is_dir():
        log("set EVAL_RESEARCH_REPO to the target checkout")
        return 2
    target_python = Path(
        args.target_python or os.environ.get("EVAL_RESEARCH_PYTHON", "")
    ).expanduser()
    if not target_python.exists():
        candidate = target / ".venv" / "Scripts" / "python.exe"
        if not candidate.exists():
            candidate = target / ".venv" / "bin" / "python"
        target_python = candidate
    if not target_python.exists():
        log("target venv python not found; set EVAL_RESEARCH_PYTHON")
        return 2
    preflight = subprocess.run(
        [str(target_python), "-c", "import trafilatura; import uvicorn"],
        capture_output=True,
        text=True,
        check=False,
    )
    if preflight.returncode != 0:
        log("target venv is missing the scrape extra (trafilatura) or uvicorn")
        log("install it with: uv pip install --python <target-python> 'trafilatura>=2.2,<3'")
        return 2

    key_files = [env_file, target / ".env"]
    openrouter_env = args.openrouter_env_file or os.environ.get(
        "EVAL_OPENROUTER_ENV_FILE", ""
    )
    if openrouter_env:
        key_files.insert(0, Path(openrouter_env))
    openrouter_key = resolve_key("OPENROUTER_API_KEY", key_files, provider="openrouter")
    if not openrouter_key:
        log("OpenRouter key not found; set OPENROUTER_API_KEY or EVAL_ENV_FILE")
        return 2

    free_model = args.free_model or DEFAULT_FREE_MODEL
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    tag = f"research-service-local-{stamp}"
    proxy_port = args.proxy_port or free_port()
    shim_port = free_port()
    workroot = store / "research-service-work"
    workroot.mkdir(parents=True, exist_ok=True)
    artifacts = store / "research-service-artifacts"
    artifacts.mkdir(parents=True, exist_ok=True)
    proxy_log = (store / "research-service-proxy.log").open("w", encoding="utf-8")

    proxy = subprocess.Popen(
        [
            str(eval_bin),
            "proxy",
            "--port",
            str(proxy_port),
            "--key-env",
            "OPENROUTER_API_KEY",
            "--project",
            "research-agents",
            "--tag",
            tag,
            "--store",
            str(store),
            "--provider",
            "openrouter",
            "--max-requests",
            str(args.max_requests),
            "--upstream-timeout-s",
            "180",
            "--override",
            f"openai/gpt-4o-mini={free_model}",
            "--override",
            f"openrouter/openai/gpt-4o-mini={free_model}",
            "--override",
            f"gpt-4o-mini={free_model}",
            "https://openrouter.ai/api/v1",
        ],
        env={**os.environ, "OPENROUTER_API_KEY": openrouter_key},
        stdout=proxy_log,
        stderr=subprocess.STDOUT,
    )
    shim_log = (store / "research-service-shim.log").open("w", encoding="utf-8")
    shim = subprocess.Popen(
        [
            sys.executable,
            str(SHIM),
            "--port",
            str(shim_port),
            "--upstream",
            f"http://127.0.0.1:{proxy_port}/v1",
        ],
        stdout=shim_log,
        stderr=subprocess.STDOUT,
    )
    try:
        time.sleep(0.5)
        env = os.environ.copy()
        env.update(
            {
                "EVAL_RESEARCH_SERVICE_LANE": "local",
                "EVAL_RESEARCH_LOCAL_SERVICE": "1",
                "EVAL_RESEARCH_REPO": str(target),
                "EVAL_RESEARCH_PYTHON": str(target_python),
                "EVAL_RESEARCH_OPENROUTER_BASE": f"http://127.0.0.1:{shim_port}/v1",
                "EVAL_RESEARCH_SERVICE_WORKROOT": str(workroot),
                "EVAL_RESEARCH_USAGE_STORE": str(store),
                "EVAL_RESEARCH_USAGE_PROJECT": "research-agents",
                "EVAL_RESEARCH_USAGE_TAG": tag,
                "EVAL_RESEARCH_REQUIRE_FREE_MODELS": "1",
                "EVAL_FREE_MODEL": free_model,
                "EVAL_FREE_MODEL_ALLOWLIST": free_model,
                "EVAL_RESEARCH_ARTIFACT_OUT": str(artifacts),
                "EVAL_RESEARCH_TRACE_ENDPOINT": (
                    args.otel_endpoint.rstrip("/") + "/v1/traces" if args.otel_endpoint else ""
                ),
                "EVAL_PYTHON": sys.executable,
            }
        )
        if args.otel_endpoint:
            env["EVAL_OTEL_ENDPOINT"] = args.otel_endpoint
        if run_probe(env, eval_bin) != 0:
            log("local service health/readiness probe failed")
            return 2
        return run_suite(
            env,
            eval_bin,
            args.ref or "research-service-local",
            store,
            lane="local-service",
            free_only=True,
        )
    finally:
        for proc in (shim, proxy):
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        shim_log.close()
        proxy_log.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("local", "remote"), default="local")
    parser.add_argument("--target-repo", default="")
    parser.add_argument("--target-python", default="")
    parser.add_argument("--env-file", default="")
    parser.add_argument("--openrouter-env-file", default="")
    parser.add_argument("--service-url", default="")
    parser.add_argument("--free-model", default="")
    parser.add_argument("--proxy-port", type=int, default=0)
    parser.add_argument("--max-requests", type=int, default=80)
    parser.add_argument("--job-timeout-s", type=int, default=1200)
    parser.add_argument("--otel-endpoint", default="")
    parser.add_argument("--ref", default="")
    parser.add_argument("--store", default="")
    args = parser.parse_args(argv)

    if os.environ.get("CI"):
        log("refusing to run live service E2E in CI")
        return 2
    eval_bin = Path(os.environ.get("EVAL_BIN", str(default_eval_bin())))
    if not eval_bin.exists():
        log(f"eval binary not found: {eval_bin}")
        return 2
    store = Path(args.store or os.environ.get("EVAL_STORE", str(default_store())))
    env_file = Path(args.env_file or os.environ.get("EVAL_ENV_FILE", ROOT / ".env"))
    store.mkdir(parents=True, exist_ok=True)
    lock_path = store / ".research-service.lock"
    try:
        lock_fd = acquire_lock(lock_path)
    except RuntimeError as exc:
        log(str(exc))
        return 3
    try:
        if args.mode == "local":
            code = local_mode(args, eval_bin, store, env_file)
        else:
            code = remote_mode(args, eval_bin, store, env_file)
        if code == 0 and os.environ.get("EVAL_S3_URI"):
            subprocess.run(
                ["aws", "s3", "sync", str(store), os.environ["EVAL_S3_URI"], "--only-show-errors"],
                check=False,
            )
        return code
    finally:
        os.close(lock_fd)
        lock_path.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())

