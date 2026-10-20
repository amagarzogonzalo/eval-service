"""eval/v1 target: wrap a subprocess command and relay its exit code.

Replace `DEFAULT_COMMAND` (or pass `input.params.command`) with your tool.
Edit ONLY `run_command`; the protocol handling below is complete.

Rule: stdout is eval/v1 JSONL. Your tool's stdout goes inside the result
message; its stderr is forwarded to our stderr (logs, never parsed).
"""

import json
import os
import subprocess
import sys

#: Default command when a case does not set `input.params.command`.
DEFAULT_COMMAND = [sys.executable, "-c", "print('hello from cli')"]


def expand(token):
    """Support `$PYTHON` (this interpreter) and `$VAR` from the environment."""
    if token == "$PYTHON":
        return sys.executable
    if token.startswith("$"):
        return os.environ.get(token[1:], token)
    return token


def run_command(case):
    """Run the target and return (exit_code, stdout, stderr).

    This is the one function you replace. Anything you can spawn works.
    `input.params` may set `command` (argv list), `cwd`, and `env`
    (extra variables merged over the process environment).
    """
    params = (case.get("input") or {}).get("params") or {}
    raw = params.get("command") or DEFAULT_COMMAND
    command = [expand(part) for part in raw]
    cwd = expand(params["cwd"]) if params.get("cwd") else None
    env = None
    if isinstance(params.get("env"), dict):
        env = {**os.environ, **params["env"]}
    try:
        proc = subprocess.run(
            command, capture_output=True, text=True, check=False, cwd=cwd, env=env
        )
    except OSError as error:
        # A target that cannot even start is a handled failure, not a crash.
        return 127, "", f"cannot run {command!r}: {error}"
    return proc.returncode, proc.stdout, proc.stderr


def handle_case(case):
    code, out, err = run_command(case)
    if err:
        sys.stderr.write(err)
        sys.stderr.flush()
    if code != 0:
        # Relay the real exit code: the engine records Crash(code) and a
        # suite grades it with an `exit_code` grader. Do NOT swallow it.
        return {
            "type": "case_error",
            "case_id": case["case_id"],
            "code": f"exit_{code}",
            "message": (err or out)[-500:] or f"command exited {code}",
            "retriable": False,
        }
    return {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {"text": out.strip(), "files": [], "data": None},
    }


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
                sys.stderr.write(f"version mismatch: {msg.get('protocol')}\n")
                sys.exit(2)
            mode = msg.get("mode", "worker")
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        print(json.dumps(handle_case(msg)), flush=True)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
