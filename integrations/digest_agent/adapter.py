"""Thin eval/v1 adapter around the real digest_agent orchestrator.

Drives the project's own LangGraph orchestrator. Two modes:

- offline (default): through the project's own fakes (FakeActivitySource,
  FakeLLMProvider, FakeDocumentRenderer, FakeNotifier);
- real services (``real_services: true``): through the target's real
  GitHub adapter and ReportLab renderer, its real SMTP/Resend notifier,
  and the live provider in this integration through eval-proxy.

Nothing in the digest_agent repository is modified. The adapter also
propagates W3C trace context to the driver and starts a local SMTP test
inbox for the notifier path (a real SMTP exchange, persisted as .eml
files + an index; external SMTP is used when SMTP_HOST is set).

Case contract (``input.params``):

- ``python``: absolute path to the project's venv python.
- ``src``: absolute path to the project's ``src/`` dir.
- ``scenario``: ``empty`` | ``single-pr`` | ``github`` |
  ``github-fixture`` | ``bad-repo``.
- ``live_bench``: run the live provider (``--live``).
- ``real_services``: forbid fake activity/renderer/notifier.
- ``fixture`` / ``output_dir`` / ``inbox``: optional paths ($VAR ok).
- ``traceparent``: optional W3C parent context.

Exit 0 carries the final digest state as JSON (``output.text`` is the
summary, ``output.data`` the full state plus side-effect evidence). A
nonzero driver exit is relayed as this adapter's exit code (Crash
observation with the real code); adapter-side timeouts are handled
``case_error``s.
"""

import json
import os
import secrets
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TIMEOUT_S = int(os.environ.get("EVAL_DIGEST_CASE_TIMEOUT_S", "420"))
SCENARIOS = ("empty", "single-pr", "github", "github-fixture", "bad-repo")


def emit(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def log(message):
    sys.stderr.write(f"adapter: {message}\n")
    sys.stderr.flush()


def expand(value):
    return os.path.expandvars(value) if value else value


def base_env(src):
    env = {"PYTHONPATH": src}
    forwarded = (
        "PATH", "PATHEXT", "SystemRoot", "SystemDrive", "TEMP", "TMP",
        "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA",
        "USERNAME", "COMPUTERNAME", "OS", "HOME", "USER", "LOGNAME",
        "TMPDIR", "LANG", "SSL_CERT_FILE", "SSL_CERT_DIR",
    )
    for key in forwarded:
        if key in os.environ:
            env[key] = os.environ[key]
    # Forward the documented namespaces so the driver inherits live
    # credentials, budgets, mock transport, usage/trace recording paths,
    # and an externally configured SMTP/Resend notifier.
    forward_prefixes = (
        "EVAL_DIGEST_", "EVAL_USAGE_", "EVAL_TRACE_", "EVAL_OTEL_",
        "SMTP_", "GMAIL_",
    )
    for key, value in os.environ.items():
        if key.startswith(forward_prefixes):
            env[key] = value
    for key in ("TRACEPARENT",):
        if key in os.environ:
            env[key] = os.environ[key]
    return env


class SMTPInbox:
    """Runs the local SMTP test sink for one case (real SMTP endpoint)."""

    def __init__(self, directory, user, password):
        self.directory = directory
        self.user = user
        self.password = password
        self.process = None
        self.ready = {}

    def start(self):
        os.makedirs(self.directory, exist_ok=True)
        ready_path = os.path.join(self.directory, "ready.json")
        try:
            os.remove(ready_path)
        except OSError:
            pass
        argv = [
            sys.executable, os.path.join(HERE, "smtp_sink.py"),
            "--out", self.directory,
            "--port", "0",
            "--user", self.user,
            "--password", self.password,
            "--ready-file", ready_path,
        ]
        self.process = subprocess.Popen(
            argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        deadline = time.time() + 10
        while time.time() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(
                    f"SMTP test sink exited with code {self.process.returncode}"
                )
            if os.path.isfile(ready_path):
                with open(ready_path, encoding="utf-8") as handle:
                    self.ready = json.load(handle)
                return self.ready
            time.sleep(0.05)
        raise RuntimeError("SMTP test sink did not become ready")

    def stop(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.process = None


def trace_env(case, case_span):
    env = {"EVAL_TRACEPARENT": case_span.traceparent()}
    parent = (case.get("input") or {}).get("params", {}).get("traceparent")
    if parent:
        env["EVAL_TRACEPARENT"] = expand(parent)
    return env


def grade_evidence(data):
    """Adapter-side grade evidence; the engine's graders stay authoritative."""
    summary = bool((data.get("summary") or "").strip())
    outcome = data.get("outcome", "")
    email_sent = bool(data.get("email_sent"))
    pdf_ok = bool(data.get("pdf_ok"))
    receipts = int(data.get("notifier_receipts", 0) or 0)
    invariants = {
        "success_has_summary": (outcome != "success") or summary,
        "pdf_present_when_notified": (not email_sent) or pdf_ok,
        "receipt_matches_send": (not email_sent) or receipts >= 1,
        "no_orphan_message": email_sent or receipts == 0,
    }
    failed = [name for name, ok in invariants.items() if not ok]
    return {
        "outcome": outcome,
        "has_summary": summary,
        "email_sent": email_sent,
        "pdf_ok": pdf_ok,
        "notifier_receipts": receipts,
        "activity_prs": int(data.get("activity_prs", 0) or 0),
        "trace_id_present": bool(data.get("trace_id")),
        "invariants_ok": not failed,
        "invariants_failed": ",".join(failed) or "none",
    }


def mark_error(span, **attributes):
    span.set(**attributes)
    span.status = "error"


def handle_case(case, tracer):
    params = (case.get("input") or {}).get("params") or {}
    for key in ("python", "src", "scenario"):
        if key not in params:
            emit({
                "type": "case_error", "case_id": case["case_id"],
                "code": "bad_params", "message": f"missing param: {key}",
                "retriable": False,
            })
            return
    python = expand(params.get("python", ""))
    src = expand(params.get("src", ""))
    if not python or "$" in python or not src or "$" in src:
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "bad_params",
            "message": "unresolved python/src path: check the documented env vars",
            "retriable": False,
        })
        return
    if params["scenario"] not in SCENARIOS:
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": "bad_params",
            "message": f"unknown scenario: {params['scenario']!r}",
            "retriable": False,
        })
        return

    real_services = bool(params.get("real_services"))
    live = bool(params.get("live_bench"))
    with tracer.scope(
        "digest.case", case_id=case["case_id"],
        scenario=params["scenario"], real_services=real_services, live=live,
    ) as case_span:
        env = base_env(src)
        env.update(trace_env(case, case_span))

        output_dir = expand(params.get("output_dir") or os.environ.get(
            "EVAL_DIGEST_OUTPUT_DIR", ""))
        if output_dir:
            output_dir = os.path.abspath(output_dir)
            env["EVAL_DIGEST_OUTPUT_DIR"] = output_dir
        fixture = expand(params.get("fixture") or "")
        if fixture:
            if "$" in fixture or not os.path.isfile(fixture):
                mark_error(case_span, error_type="bad_fixture")
                emit({
                    "type": "case_error", "case_id": case["case_id"],
                    "code": "bad_params",
                    "message": "activity fixture not found: provide "
                               "EVAL_DIGEST_E2E_FIXTURE (repo-relative path)",
                    "retriable": False,
                })
                return
            env["EVAL_DIGEST_ACTIVITY_FIXTURE"] = os.path.abspath(fixture)
        inbox = expand(params.get("inbox") or os.environ.get(
            "EVAL_DIGEST_SMTP_INBOX", ""))
        if inbox:
            inbox = os.path.abspath(inbox)
            env["EVAL_DIGEST_SMTP_INBOX"] = inbox

        sink = None
        notifier = os.environ.get(
            "EVAL_DIGEST_NOTIFIER", "smtp" if real_services else "fake")
        renderer = os.environ.get(
            "EVAL_DIGEST_RENDERER", "reportlab" if real_services else "fake")
        env["EVAL_DIGEST_NOTIFIER"] = notifier
        env["EVAL_DIGEST_RENDERER"] = renderer
        external_smtp = bool(os.environ.get("SMTP_HOST"))
        if real_services and notifier == "smtp" and not external_smtp:
            inbox = inbox or os.path.join(
                output_dir or os.path.join(
                    tempfile.gettempdir(), "digest-e2e"),
                "mailbox",
            )
            inbox = os.path.abspath(inbox)
            env["EVAL_DIGEST_SMTP_INBOX"] = inbox
            user = "eval-e2e"
            password = secrets.token_hex(12)
            sink = SMTPInbox(inbox, user, password)
            try:
                ready = sink.start()
            except OSError as error:
                mark_error(case_span, error_type="smtp_sink_unavailable")
                emit({
                    "type": "case_error", "case_id": case["case_id"],
                    "code": "smtp_sink_unavailable",
                    "message": str(error)[:300], "retriable": True,
                })
                return
            env["SMTP_HOST"] = ready["host"]
            env["SMTP_PORT"] = str(ready["port"])
            env["SMTP_USER"] = user
            env["SMTP_PASSWORD"] = password
            log("SMTP test inbox on {}:{}".format(ready["host"], ready["port"]))

        try:
            argv = [python, os.path.join(HERE, "driver.py"), params["scenario"]]
            for flag in params.get("flags") or []:
                if flag in ("--weak", "--fail-fetch", "--live"):
                    argv.append(flag)
            if live:
                argv.append("--live")
            if real_services:
                argv.append("--real-services")
            usage_out = params.get("usage_out") or os.environ.get("EVAL_USAGE_OUT", "")
            if usage_out:
                argv += ["--usage-out", expand(usage_out),
                         "--task-id", case["case_id"]]
            proc = subprocess.run(
                argv,
                env=env,
                capture_output=True, text=True, timeout=TIMEOUT_S, check=False,
            )
        except subprocess.TimeoutExpired:
            if sink:
                sink.stop()
            mark_error(case_span, timed_out=True)
            emit({
                "type": "case_error", "case_id": case["case_id"],
                "code": "adapter_timeout",
                "message": f"digest driver exceeded {TIMEOUT_S}s",
                "retriable": True,
            })
            return
        finally:
            if sink:
                sink.stop()

        if proc.returncode != 0:
            mark_error(case_span, driver_exit_code=proc.returncode)
            sys.stderr.write((proc.stderr or "")[-2000:])
            sys.stderr.flush()
            sys.exit(proc.returncode)
        try:
            state = json.loads((proc.stdout or "").strip().splitlines()[-1])
        except (ValueError, IndexError):
            mark_error(case_span, driver_output=False)
            emit({
                "type": "case_error", "case_id": case["case_id"],
                "code": "bad_driver_output",
                "message": "driver printed no JSON state",
                "retriable": False,
            })
            return

        evidence = grade_evidence(state)
        with tracer.scope(
            "digest.grade", parent=case_span, **evidence,
        ):
            pass
        case_span.set(**evidence)
        if "trace_id" not in state and (real_services or live):
            state["trace_id"] = tracer.trace_id
        emit({
            "type": "result", "case_id": case["case_id"], "ok": True,
            "output": {"text": state.get("summary", ""), "data": state, "files": []},
        })


def main():
    mode = "worker"
    handshook = False
    from eval_trace import Tracer

    tracer = Tracer.from_env()
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
            tracer.flush()
            return
        handle_case(msg, tracer)
        if mode == "oneshot":
            tracer.flush()
            return


if __name__ == "__main__":
    main()


