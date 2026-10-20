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
- ``mode``: ``replay`` only for now; live recording would write the target's
  cassettes and needs the proxy, so it is deliberately not wired here.
"""

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

DEFAULT_CASE = "research"
DEFAULT_TIMEOUT_S = 600
DEFAULT_LIVE_TIMEOUT_S = 1200

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


def handle_case(case):
    case_id = case["case_id"]
    params = (case.get("input") or {}).get("params") or {}
    for key in ("python", "repo"):
        if not params.get(key):
            fail(case_id, "bad_params", f"missing param: {key}")
            return

    python = os.path.expandvars(params["python"])
    repo = os.path.expandvars(params["repo"])
    target_case = os.path.expandvars(params.get("case", DEFAULT_CASE))
    mode = params.get("mode", "replay")
    if "$" in python or "$" in repo:
        fail(case_id, "bad_params", "unresolved python/repo path")
        return
    if mode not in ("replay", "live"):
        fail(
            case_id,
            "unsupported_mode",
            "mode must be replay or live",
        )
        return

    if mode == "live":
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
        with tempfile.TemporaryDirectory(prefix="eval-research-live-") as workdir:
            try:
                proc = subprocess.run(
                    [
                        python,
                        str(live_runner),
                        "--repo",
                        repo,
                        "--case",
                        target_case,
                        "--endpoint",
                        endpoint,
                        "--model",
                        model,
                    ],
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
        summary = (
            f"{status.upper()} {target_case} live "
            f"({data.get('artifact_chars', 0)} chars, "
            f"{data.get('nsources', 0)} sources)"
        )
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
