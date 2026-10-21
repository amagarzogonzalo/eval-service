"""eval/v1 adapter around research-orchestration-agents' cassette eval harness.

The target already owns a deterministic replay harness
(``evals/runner.py``): cassettes pin every LLM/search/scrape call, and its
invariants/budgets decide whether the research run is sound. This adapter
runs that harness in a temporary working directory (the target is never
written to) and forwards the structured result to eval-service graders.

Case params:
- ``python``: target venv python (required).
- ``repo``: target repository root (required).
- ``case``: cassette case id (default ``research``).
- ``mode``: ``replay`` (target cassettes), ``live`` (live model calls through
  eval proxy, recorded retrieval), ``live_e2e`` (in-process engine debug lane),
  or ``live_service`` (black-box target service: local uvicorn or Render).
- ``dataset``/``topic``: dataset JSON and topic id.
- ``traceparent``/``trace_endpoint``: W3C trace context and OTLP/HTTP sink.
- ``live_service`` params: ``service_url``, ``lane``, ``health_only``;
  the bearer token comes from ``EVAL_RESEARCH_SERVICE_TOKEN`` or
  ``RENDER_API_KEY`` and is never placed in argv.
"""

import json
import os
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

DEFAULT_CASE = "research"
DEFAULT_TIMEOUT_S = 600
DEFAULT_LIVE_TIMEOUT_S = 1200
DEFAULT_SERVICE_TIMEOUT_S = 1500
DEFAULT_DATASET = "datasets/research_e2e/topics.json"
DEFAULT_TOPIC = "rag-hallucinations"

BASE_ENV_KEYS = (
    "PATH", "PATHEXT", "SystemRoot", "SystemDrive", "TEMP", "TMP",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA",
    "USERNAME", "COMPUTERNAME", "OS", "HOME", "USER", "LOGNAME",
    "TMPDIR", "LANG",
)


def emit(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def base_env():
    env = {key: os.environ[key] for key in BASE_ENV_KEYS if key in os.environ}
    for key, value in os.environ.items():
        if key.startswith(("EVAL_RESEARCH_", "EVAL_USAGE_")):
            env[key] = value
    return env


def parse_results(stdout):
    """Return the JSON array printed by the target's --json mode."""
    start = stdout.find("[")
    if start < 0:
        return None
    try:
        value, _ = json.JSONDecoder().raw_decode(stdout[start:])
    except ValueError:
        return None
    return value if isinstance(value, list) else None


def parse_live(stdout):
    """Return the JSON object printed by the live runner."""
    for line in reversed((stdout or "").splitlines()):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            return json.loads(line)
        except ValueError:
            continue
    return None


def fail(case_id, code, message, retriable=False):
    emit({
        "type": "case_error",
        "case_id": case_id,
        "code": code,
        "message": message,
        "retriable": retriable,
    })


def _absolute_env_path(name):
    value = os.path.expandvars(os.environ.get(name, ""))
    if not value or "$" in value:
        return ""
    if not os.path.isabs(value):
        return str(Path.cwd() / value)
    return value


def _free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def _service_env(repo: str, proxy_base: str, token: str, tag: str) -> dict:
    env = base_env()
    env.update({
        "PYTHONPATH": os.pathsep.join([str(Path(repo) / "src"), repo]),
        "PYTHONDONTWRITEBYTECODE": "1",
        "JOBS_KIND": "memory",
        "API_TOKEN": token,
        "ALLOW_ANONYMOUS": "0",
        "MAX_CONCURRENT_JOBS": "1",
        "MAX_QUEUED_JOBS": "1",
        "MAX_ACTIVE_JOBS_PER_CALLER": "1",
        "RATE_LIMIT_PER_MINUTE": "0",
        "JOB_TIMEOUT_SECONDS": "1200",
        "CLEANUP_AFTER_RUN": "false",
        "ARTIFACT_STORE_KIND": "local",
        "CHECKPOINTER_KIND": "file",
        "STORE_KIND": "memory",
        "WIKI_STORE_KIND": "local",
        "OPENROUTER_API_KEY": "dummy-eval-proxy",
        "OPENROUTER_API_BASE": proxy_base,
        "GROQ_API_KEY": "",
        "GEMINI_API_KEY": "",
        "MISTRAL_API_KEY": "",
        "OTEL_SERVICE_NAME": "research-orchestration-agents-local",
        "OTEL_RESOURCE_ATTRIBUTES": f"eval.lane=service_local,service.run_tag={tag}",
    })
    otel = os.environ.get("EVAL_OTEL_ENDPOINT", "").strip()
    if otel:
        env["OTEL_TRACES_EXPORTER"] = "otlp"
        env["OTEL_EXPORTER_OTLP_ENDPOINT"] = otel.rstrip("/") + "/v1/traces"
    else:
        env["OTEL_TRACES_EXPORTER"] = "none"
    return env


def _start_local_service(repo: str, python: str) -> tuple[subprocess.Popen, str, str, Path]:
    proxy_base = os.environ.get("EVAL_RESEARCH_OPENROUTER_BASE", "").strip()
    if not proxy_base:
        raise RuntimeError("EVAL_RESEARCH_OPENROUTER_BASE is required for local service")
    token = secrets.token_hex(24)
    tag = os.environ.get("EVAL_RESEARCH_USAGE_TAG", "")
    port = _free_port()
    workroot = Path(os.environ.get("EVAL_RESEARCH_SERVICE_WORKROOT", ""))
    if not str(workroot):
        workroot = Path(tempfile.gettempdir())
    workroot.mkdir(parents=True, exist_ok=True)
    workdir = Path(tempfile.mkdtemp(prefix="eval-research-service-", dir=str(workroot)))
    log_path = workdir / "service.log"
    log = log_path.open("w", encoding="utf-8")
    env = _service_env(repo, proxy_base, token, tag)
    proc = subprocess.Popen(
        [python, "-m", "uvicorn", "knowledge_engine.service.api:app",
         "--host", "127.0.0.1", "--port", str(port)],
        cwd=str(workdir),
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    log.close()
    deadline = time.time() + 90
    while time.time() < deadline:
        if proc.poll() is not None:
            tail = log_path.read_text(encoding="utf-8", errors="replace")[-2000:]
            raise RuntimeError(f"local service exited {proc.returncode}: {tail}")
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1)
            return proc, f"http://127.0.0.1:{port}", token, workdir
        except OSError:
            time.sleep(0.5)
    proc.terminate()
    raise RuntimeError("local service did not become healthy in 90s")


def handle_service(case_id, params):
    """Run the black-box target service contract (local or deployed)."""
    repo_root = Path(__file__).resolve().parents[2]
    lane = os.path.expandvars(str(params.get("lane") or ""))
    if not lane or lane.startswith("$"):
        lane = os.environ.get("EVAL_RESEARCH_SERVICE_LANE", "remote")
    if lane not in ("local", "remote"):
        fail(case_id, "bad_params", f"invalid service lane: {lane}")
        return
    health_only = bool(params.get("health_only"))
    local_auto = lane == "local" and os.environ.get(
        "EVAL_RESEARCH_LOCAL_SERVICE", ""
    ).strip().lower() in ("1", "true", "yes", "on")
    service_url = ""
    token = ""
    if local_auto:
        if not os.environ.get("EVAL_RESEARCH_REPO") or not os.environ.get(
            "EVAL_RESEARCH_PYTHON"
        ):
            fail(case_id, "bad_params", "local service needs EVAL_RESEARCH_REPO/PYTHON")
            return
    else:
        service_url = os.path.expandvars(
            params.get("service_url") or os.environ.get("EVAL_RESEARCH_SERVICE_URL", "")
        )
        if not service_url or "$" in service_url:
            fail(case_id, "bad_params", "missing param/env: EVAL_RESEARCH_SERVICE_URL")
            return
        token = os.environ.get("EVAL_RESEARCH_SERVICE_TOKEN", "") or os.environ.get(
            "RENDER_API_KEY", ""
        )
        if not health_only and not token:
            fail(
                case_id,
                "missing_service_token",
                "set EVAL_RESEARCH_SERVICE_TOKEN or RENDER_API_KEY",
            )
            return
    dataset = os.path.expandvars(
        params.get("dataset")
        or os.environ.get("EVAL_RESEARCH_DATASET")
        or str(repo_root / DEFAULT_DATASET)
    )
    topic = os.path.expandvars(
        params.get("topic") or os.environ.get("EVAL_RESEARCH_TOPIC", DEFAULT_TOPIC)
    )
    if "$" in dataset:
        dataset = str(repo_root / DEFAULT_DATASET)
    if not os.path.isabs(dataset):
        dataset = str(repo_root / dataset)
    if "$" in topic:
        topic = DEFAULT_TOPIC
    traceparent = os.path.expandvars(
        params.get("traceparent")
        or os.environ.get("EVAL_RESEARCH_TRACEPARENT")
        or os.environ.get("EVAL_TRACEPARENT")
        or os.environ.get("TRACEPARENT")
        or ""
    )
    trace_endpoint = os.path.expandvars(
        params.get("trace_endpoint")
        or os.environ.get("EVAL_RESEARCH_TRACE_ENDPOINT")
        or os.environ.get("EVAL_OTEL_ENDPOINT")
        or ""
    )
    if "$" in traceparent:
        traceparent = ""
    if "$" in trace_endpoint:
        trace_endpoint = ""
    runner = Path(__file__).resolve().with_name("service_runner.py")
    if not runner.is_file():
        fail(case_id, "bad_params", f"service runner missing: {runner}")
        return
    timeout_s = int(params.get("timeout_s", DEFAULT_SERVICE_TIMEOUT_S))
    local_proc = None
    local_workdir = None
    if local_auto:
        try:
            local_proc, service_url, token, local_workdir = _start_local_service(
                os.environ["EVAL_RESEARCH_REPO"], os.environ["EVAL_RESEARCH_PYTHON"]
            )
        except (RuntimeError, OSError) as exc:
            fail(case_id, "local_service_failed", str(exc)[:1000])
            return
    command = [
        sys.executable,
        str(runner),
        "--base-url",
        service_url,
        "--lane",
        lane,
        "--dataset",
        dataset,
        "--topic",
        topic,
        "--timeout-s",
        str(timeout_s),
        "--poll-s",
        str(params.get("poll_s", 2)),
        "--poll-max-s",
        str(params.get("poll_max_s", 10)),
        "--usage-store",
        _absolute_env_path("EVAL_RESEARCH_USAGE_STORE"),
        "--usage-project",
        os.environ.get("EVAL_RESEARCH_USAGE_PROJECT", "research-agents"),
        "--usage-tag",
        os.path.expandvars(os.environ.get("EVAL_RESEARCH_USAGE_TAG", "")),
    ]
    if health_only:
        command.append("--health-only")
    if params.get("thread_id"):
        command += ["--thread-id", str(params["thread_id"])]
    artifact_out = os.path.expandvars(
        params.get("artifact_out") or os.environ.get("EVAL_RESEARCH_ARTIFACT_OUT", "")
    )
    if artifact_out and "$" not in artifact_out:
        if not os.path.isabs(artifact_out):
            artifact_out = str(Path.cwd() / artifact_out)
        command += ["--artifact-out", artifact_out]
    if traceparent:
        command += ["--traceparent", traceparent]
    if os.environ.get("EVAL_TRACE_ID"):
        command += ["--trace-id", os.environ["EVAL_TRACE_ID"]]
    if trace_endpoint:
        command += ["--trace-endpoint", trace_endpoint]
    require_free = bool(params.get("require_free_models")) or os.environ.get(
        "EVAL_RESEARCH_REQUIRE_FREE_MODELS", ""
    ).strip().lower() in ("1", "true", "yes", "on")
    allowlist = os.path.expandvars(
        params.get("free_model_allowlist")
        or os.environ.get("EVAL_FREE_MODEL_ALLOWLIST", "")
        or os.environ.get("EVAL_FREE_MODEL", "")
    )
    if require_free:
        command.append("--require-free-models")
    if allowlist:
        command += ["--free-model-allowlist", allowlist]
    env = base_env()
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["PYTHONUTF8"] = "1"
    if token:
        env["EVAL_RESEARCH_SERVICE_TOKEN"] = token
    with tempfile.TemporaryDirectory(prefix="eval-research-service-") as workdir:
        try:
            proc = subprocess.run(
                command,
                cwd=workdir,
                env=env,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout_s + 30,
                check=False,
            )
        except subprocess.TimeoutExpired:
            fail(
                case_id,
                "research_service_timeout",
                f"service run exceeded {timeout_s}s",
                retriable=True,
            )
            return
        finally:
            if local_proc is not None:
                local_proc.terminate()
                try:
                    local_proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    local_proc.kill()
            if local_workdir is not None and os.environ.get(
                "EVAL_RESEARCH_KEEP_WORKDIR", ""
            ).strip().lower() not in ("1", "true", "yes", "on"):
                shutil.rmtree(local_workdir, ignore_errors=True)
    data = parse_live(proc.stdout or "")
    if data is None:
        stderr = (proc.stderr or "")[-2000:]
        fail(
            case_id,
            "research_service_failed",
            f"service runner exit {proc.returncode}; no JSON. stderr: {stderr}",
        )
        return
    failures = data.get("failures") or []
    status = data.get("status", "fail")
    summary = f"{status.upper()} {data.get('topic', topic)} {lane}"
    task_id = data.get("task_id")
    if task_id:
        summary += f" task={task_id}"
    if data.get("service", {}).get("e2e_ms"):
        summary += f" e2e={data['service']['e2e_ms']}ms"
    if failures:
        summary += " | " + " | ".join(str(item) for item in failures[:5])
    emit({
        "type": "result",
        "case_id": case_id,
        "ok": True,
        "output": {"text": summary, "data": data, "files": []},
    })


def handle_case(case):
    case_id = case["case_id"]
    params = (case.get("input") or {}).get("params") or {}
    mode = params.get("mode", "replay")
    if mode == "live_service":
        handle_service(case_id, params)
        return
    for key in ("python", "repo"):
        if not params.get(key):
            fail(case_id, "bad_params", f"missing param: {key}")
            return

    python = os.path.expandvars(params["python"])
    repo = os.path.expandvars(params["repo"])
    target_case = os.path.expandvars(params.get("case", DEFAULT_CASE))
    if "$" in python or "$" in repo:
        fail(case_id, "bad_params", "unresolved python/repo path")
        return
    if mode not in ("replay", "live", "live_e2e"):
        fail(
            case_id,
            "unsupported_mode",
            "mode must be replay, live or live_e2e",
        )
        return

    if mode in ("live", "live_e2e"):
        for key in ("endpoint", "model"):
            if not params.get(key):
                fail(case_id, "bad_params", f"missing param: {key}")
                return
        endpoint = os.path.expandvars(params["endpoint"])
        model = os.path.expandvars(params["model"])
        if "$" in endpoint or "$" in model:
            fail(case_id, "bad_params", "unresolved endpoint/model")
            return
        live_runner = Path(__file__).resolve().with_name("live_runner.py")
        if not live_runner.is_file():
            fail(case_id, "bad_params", f"live runner missing: {live_runner}")
            return
        env = base_env()
        env["PYTHONPATH"] = os.pathsep.join(
            [str(Path(repo) / "src"), repo, str(Path(repo) / "evals")]
        )
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env["PYTHONUTF8"] = "1"
        timeout_s = int(params.get("timeout_s", DEFAULT_LIVE_TIMEOUT_S))
        command = [
            python,
            str(live_runner),
            "--repo",
            repo,
            "--endpoint",
            endpoint,
            "--model",
            model,
        ]
        if mode == "live_e2e":
            repo_root = Path(__file__).resolve().parents[2]
            dataset = os.path.expandvars(
                params.get("dataset")
                or os.environ.get("EVAL_RESEARCH_DATASET")
                or str(repo_root / DEFAULT_DATASET)
            )
            topic = os.path.expandvars(
                params.get("topic") or os.environ.get("EVAL_RESEARCH_TOPIC", DEFAULT_TOPIC)
            )
            if "$" in dataset:
                dataset = str(repo_root / DEFAULT_DATASET)
            if not os.path.isabs(dataset):
                dataset = str(repo_root / dataset)
            if "$" in topic:
                topic = DEFAULT_TOPIC
            traceparent = os.path.expandvars(
                params.get("traceparent")
                or os.environ.get("EVAL_RESEARCH_TRACEPARENT")
                or os.environ.get("EVAL_TRACEPARENT")
                or os.environ.get("TRACEPARENT")
                or ""
            )
            trace_endpoint = os.path.expandvars(
                params.get("trace_endpoint")
                or os.environ.get("EVAL_RESEARCH_TRACE_ENDPOINT")
                or os.environ.get("EVAL_OTEL_ENDPOINT")
                or os.environ.get("EVAL_TRACE_OTLP_ENDPOINT")
                or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
                or ""
            )
            if "$" in traceparent:
                traceparent = ""
            if "$" in trace_endpoint:
                trace_endpoint = ""
            if trace_endpoint and not trace_endpoint.rstrip("/").endswith("/v1/traces"):
                trace_endpoint = trace_endpoint.rstrip("/") + "/v1/traces"
            command += ["--mode", "live_e2e", "--dataset", dataset, "--topic", topic]
            if params.get("max_sources"):
                command += ["--max-sources", str(int(params["max_sources"]))]
            if params.get("max_scrapes"):
                command += ["--max-scrapes", str(int(params["max_scrapes"]))]
            artifact_out = os.path.expandvars(
                params.get("artifact_out")
                or os.environ.get("EVAL_RESEARCH_ARTIFACT_OUT", "")
            )
            if "$" in artifact_out:
                artifact_out = ""
            if traceparent:
                command += ["--traceparent", traceparent]
            if trace_endpoint:
                command += ["--trace-endpoint", trace_endpoint]
            if artifact_out:
                command += ["--artifact-out", artifact_out]
        else:
            command += ["--case", target_case]
        with tempfile.TemporaryDirectory(prefix="eval-research-live-") as workdir:
            try:
                proc = subprocess.run(
                    command,
                    cwd=workdir,
                    env=env,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    timeout=timeout_s,
                    check=False,
                )
            except subprocess.TimeoutExpired:
                fail(
                    case_id,
                    "research_live_timeout",
                    f"live research exceeded {timeout_s}s",
                    retriable=True,
                )
                return
        data = parse_live(proc.stdout or "")
        if data is None:
            stderr = (proc.stderr or "")[-2000:]
            fail(
                case_id,
                "research_live_failed",
                f"live runner exit {proc.returncode}; no JSON. stderr: {stderr}",
            )
            return
        failures = data.get("failures") or []
        status = data.get("status", "fail")
        label = data.get("topic", target_case)
        summary = (
            f"{status.upper()} {label} live "
            f"({data.get('artifact_chars', 0)} chars, "
            f"{data.get('nsources', 0)} sources, "
            f"{data.get('bodies', 0)} bodies)"
        )
        trace_id = data.get("trace_id")
        if trace_id:
            summary += f" trace={trace_id}"
        if failures:
            summary += " | " + " | ".join(str(item) for item in failures[:5])
        emit({
            "type": "result",
            "case_id": case_id,
            "ok": True,
            "output": {"text": summary, "data": data, "files": []},
        })
        return

    runner = Path(repo) / "evals" / "runner.py"
    if not runner.is_file():
        fail(case_id, "bad_params", f"target runner not found: {runner}")
        return

    env = base_env()
    env["PYTHONPATH"] = os.pathsep.join([str(Path(repo) / "src"), repo])
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["PYTHONUTF8"] = "1"
    env["EVAL_MODE"] = "replay"

    timeout_s = int(params.get("timeout_s", DEFAULT_TIMEOUT_S))
    with tempfile.TemporaryDirectory(prefix="eval-research-") as workdir:
        try:
            proc = subprocess.run(
                [python, str(runner), "-k", target_case, "--json"],
                cwd=workdir,
                env=env,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout_s,
                check=False,
            )
        except subprocess.TimeoutExpired:
            fail(
                case_id,
                "research_timeout",
                f"target replay exceeded {timeout_s}s",
                retriable=True,
            )
            return

    results = parse_results(proc.stdout or "")
    result = None
    if results:
        result = next(
            (item for item in results if item.get("id") == target_case), None
        )
    if result is None:
        stderr = (proc.stderr or "")[-2000:]
        fail(
            case_id,
            "research_runner_failed",
            f"target exit {proc.returncode}; no JSON result. stderr: {stderr}",
        )
        return

    usage = result.get("usage") or {}
    failures = result.get("failures") or []
    ok = bool(result.get("ok"))
    status = "pass" if ok else "fail"
    summary = (
        f"{status.upper()} {target_case} "
        f"({usage.get('total_tokens', 0)} tok, "
        f"{result.get('artifact_chars', 0)} chars, "
        f"{result.get('nsources', 0)} sources)"
    )
    if failures:
        summary += " | " + " | ".join(str(item) for item in failures[:5])

    data = {
        "case": result.get("id"),
        "ok": ok,
        "status": status,
        "failures": failures,
        "notes": result.get("notes") or [],
        "usage": usage,
        "artifact_chars": result.get("artifact_chars", 0),
        "nsources": result.get("nsources", 0),
        "bodies": result.get("bodies", 0),
        "digest": result.get("digest") or {},
        "judge_criteria": result.get("judge_criteria") or [],
    }
    emit({
        "type": "result",
        "case_id": case_id,
        "ok": True,
        "output": {"text": summary, "data": data, "files": []},
    })


def main():
    handshook = False
    mode = "worker"
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if not handshook:
            if message.get("protocol") != "eval/v1":
                sys.stderr.write("version mismatch\n")
                sys.exit(2)
            mode = message.get("mode", "worker")
            emit({"protocol": "eval/v1", "ready": True})
            handshook = True
            continue
        if message.get("type") == "shutdown":
            return
        handle_case(message)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
