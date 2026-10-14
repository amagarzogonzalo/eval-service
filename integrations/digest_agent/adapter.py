#!/usr/bin/env python3
"""Thin eval/v1 adapter around the real digest_agent orchestrator.

Drives the project's own LangGraph orchestrator end-to-end offline
through its own fakes (FakeActivitySource, FakeLLMProvider,
FakeDocumentRenderer, FakeNotifier). Nothing in the digest_agent
repository is modified.

Case contract (``input.params``):

- ``python``: absolute path to the project's venv python.
- ``src``: absolute path to the project's ``src/`` dir.
- ``scenario``: ``empty`` | ``single-pr`` (see driver.py).

Exit 0 carries the final digest state as JSON (``output.text`` is the
summary, ``output.data`` the full state). A nonzero driver exit is
relayed as this adapter's exit code (Crash observation with the real
code); adapter-side timeouts are handled ``case_error``s.
"""

import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TIMEOUT_S = 150


def emit(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def base_env(src):
    env = {"PYTHONPATH": src}
    for key in (
        "PATH", "PATHEXT", "SystemRoot", "SystemDrive", "TEMP", "TMP",
        "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA",
        "USERNAME", "COMPUTERNAME", "OS", "HOME", "USER", "LOGNAME",
        "TMPDIR", "LANG",
    ):
        if key in os.environ:
            env[key] = os.environ[key]
    return env


def handle_case(case):
    params = (case.get("input") or {}).get("params") or {}
    # Portable paths: $VAR references expand here (see integrations README).
    python = os.path.expandvars(params.get("python", ""))
    src = os.path.expandvars(params.get("src", ""))
    for key in ("python", "src", "scenario"):
        if key not in params:
            emit({
                "type": "case_error", "case_id": case["case_id"],
                "code": "bad_params", "message": f"missing param: {key}",
                "retriable": False,
            })
            return
    if not python or "$" in python or not src or "$" in src:
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "bad_params",
            "message": "unresolved python/src path: check the documented env vars",
            "retriable": False,
        })
        return
    if params["scenario"] not in ("empty", "single-pr"):
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "bad_params",
            "message": f"unknown scenario: {params['scenario']!r}",
            "retriable": False,
        })
        return
    try:
        argv = [python, os.path.join(HERE, "driver.py"), params["scenario"]]
        for flag in params.get("flags") or []:
            if flag in ("--weak", "--fail-fetch"):
                argv.append(flag)
        # Usage recording is opt-in: explicit param wins, otherwise the
        # environment default applies (documented in integrations README).
        usage_out = params.get("usage_out") or os.environ.get("EVAL_USAGE_OUT", "")
        if usage_out:
            argv += ["--usage-out", os.path.expandvars(usage_out),
                     "--task-id", case["case_id"]]
        proc = subprocess.run(
            argv,
            env=base_env(src),
            capture_output=True, text=True, timeout=TIMEOUT_S,
        )
    except subprocess.TimeoutExpired:
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "adapter_timeout",
            "message": f"digest driver exceeded {TIMEOUT_S}s",
            "retriable": True,
        })
        return
    if proc.returncode != 0:
        sys.stderr.write((proc.stderr or "")[-2000:])
        sys.stderr.flush()
        sys.exit(proc.returncode)
    try:
        state = json.loads((proc.stdout or "").strip().splitlines()[-1])
    except (ValueError, IndexError):
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "bad_driver_output",
            "message": "driver printed no JSON state",
            "retriable": False,
        })
        return
    emit({
        "type": "result", "case_id": case["case_id"], "ok": True,
        "output": {"text": state.get("summary", ""), "data": state, "files": []},
    })


def main():
    mode = "worker"
    handshook = False
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if not handshook:
            if msg.get("protocol") != "eval/v1":
                sys.stderr.write("version mismatch\n")
                sys.exit(2)
            mode = msg.get("mode", "worker")
            emit({"protocol": "eval/v1", "ready": True})
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        handle_case(msg)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
