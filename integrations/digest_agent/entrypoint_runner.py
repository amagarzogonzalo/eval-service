"""Run the real digest entrypoint in a temporary working directory.

This is the output-only digest path. It executes
``python -m digest_agent.main`` (the target's actual entrypoint) and
captures outer metrics only. Inner stage/token metrics are reported as
``n/a`` because the target exposes none and no proxy is used.

Free-only policy:

- the default provider is the target's Gemini free-tier adapter
  (``GEMINI_API_KEY``); the Groq free candidates remain available as an
  explicit fallback provider;
- only the selected provider key is passed; OpenRouter, Mistral, the
  other provider key, and paid overrides are removed;
- replay mode additionally installs a fixture transport that rejects any
  model outside the selected provider's reviewed free candidate list;
- preflight mode passes no provider key at all and must fail closed.

Modes (``EVAL_DIGEST_ENTRYPOINT_MODE``): ``live``, ``fixture``,
``preflight``.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import UTC, datetime

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
TIMEOUT_S = int(os.environ.get("EVAL_DIGEST_CASE_TIMEOUT_S", "420"))
FREE_GEMINI_MODELS = {
    "gemini-3.6-flash",
    "gemini-flash-lite-latest",
}
FREE_GROQ_MODELS = {
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "groq/compound-mini",
}
PROVIDERS = {
    "gemini": {
        "env": "EVAL_DIGEST_GEMINI_API_KEY",
        "dotenv": "GEMINI_API_KEY",
        "child": "GEMINI_API_KEY",
        "models": FREE_GEMINI_MODELS,
    },
    "groq": {
        "env": "EVAL_DIGEST_GROQ_API_KEY",
        "dotenv": "GROQ_API_KEY",
        "child": "GROQ_API_KEY",
        "models": FREE_GROQ_MODELS,
    },
}
BASE_ENV = (
    "PATH", "PATHEXT", "SystemRoot", "SystemDrive", "TEMP", "TMP",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA",
    "USERNAME", "COMPUTERNAME", "OS", "HOME", "USER", "LOGNAME",
    "TMPDIR", "LANG",
)
FORBIDDEN_PROVIDER_KEYS = (
    "OPENROUTER_API_KEY", "OPEN_ROUTER_API_KEY",
    "GEMINI_API_KEY", "GROQ_API_KEY", "MISTRAL_API_KEY",
)


def utc_now():
    return datetime.now(UTC).isoformat(timespec="seconds")


def log(message):
    sys.stderr.write(f"entrypoint: {message}\n")
    sys.stderr.flush()


def parse_dotenv_key(path, key_name):
    """Read exactly one named key from a dotenv file.

    The target .env may also contain paid provider keys. This parser
    returns only the requested key; every other line is ignored. It
    never logs the value.
    """
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            lines = handle.readlines()
    except OSError:
        return ""
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[7:].lstrip()
        key, separator, value = line.partition("=")
        if not separator or key.strip() != key_name:
            continue
        value = value.strip().strip('"').strip("'").strip()
        if value:
            return value
    return ""


def resolve_provider_key(provider):
    config = PROVIDERS[provider]
    key = os.environ.get(config["env"], "").strip()
    if key:
        return key, "env"
    path = os.environ.get("EVAL_DIGEST_PROVIDER_KEY_FILE", "").strip()
    if path:
        value = parse_dotenv_key(path, config["dotenv"])
        if value:
            return value, "key_file"
    return "", "none"


def child_env(src, mode, bad_repo, provider="gemini", provider_key=""):
    env = {}
    for key in BASE_ENV:
        if key in os.environ:
            env[key] = os.environ[key]
    for key, value in os.environ.items():
        if key.startswith(("SMTP_", "GMAIL_", "EVAL_TRACE_", "EVAL_OTEL_")):
            env[key] = value
    for key in FORBIDDEN_PROVIDER_KEYS:
        env.pop(key, None)
    env["PYTHONPATH"] = src
    env["DIGEST_OUTPUT_DIR"] = ""
    env["GITHUB_REPOS"] = os.environ.get(
        "EVAL_DIGEST_BAD_REPOS", "fixture/missing-repo"
    ) if bad_repo else os.environ.get(
        "EVAL_DIGEST_GITHUB_REPOS", "amagarzogonzalo/eval-service"
    )
    env["GITHUB_TOKEN"] = os.environ.get(
        "EVAL_DIGEST_GITHUB_TOKEN", "fixture-read-only-token"
    )
    env["LOOKBACK_DAYS"] = os.environ.get(
        "EVAL_DIGEST_LOOKBACK_DAYS", "7"
    )
    env["RECIPIENT_EMAIL"] = os.environ.get(
        "EVAL_DIGEST_RECIPIENT", "digest-e2e@test.invalid"
    )
    env["NOTIFIER"] = "smtp"
    env.pop("RESEND_API_KEY", None)
    env.pop("RESEND_FROM", None)
    if mode == "fixture":
        env[PROVIDERS[provider]["child"]] = "fixture-free-key"
    elif mode == "live" and provider_key:
        env[PROVIDERS[provider]["child"]] = provider_key
    return env


def copy_entrypoint_src(src, tempdir):
    """Copy the target source without any sibling .env file.

    The target calls ``load_dotenv()``, which walks up from the package
    directory. Running a copy under the temporary cwd prevents a target
    checkout's local .env from injecting provider keys (possibly paid).
    """
    isolated = os.path.join(tempdir, "src")
    shutil.copytree(
        src, isolated, dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".env", ".env.*"),
    )
    return isolated


def install_fixture_shim(tempdir, src, provider):
    shim = os.path.join(tempdir, "sitecustomize.py")
    shutil.copyfile(os.path.join(HERE, "replay_sitecustomize.py"), shim)
    fixture_github = os.path.abspath(os.environ.get(
        "EVAL_DIGEST_FIXTURE_GITHUB",
        os.path.join(ROOT, "datasets", "digest_e2e", "github_activity.json"),
    ))
    fixture_llm = os.path.abspath(os.environ.get(
        "EVAL_DIGEST_FIXTURE_LLM",
        os.path.join(ROOT, "datasets", "digest_e2e", "llm_fixture.json"),
    ))
    counts = os.path.join(tempdir, "request-counts.json")
    env = {
        "EVAL_DIGEST_FIXTURE_GITHUB": fixture_github,
        "EVAL_DIGEST_FIXTURE_LLM": fixture_llm,
        "EVAL_DIGEST_FIXTURE_COUNTS": counts,
        "EVAL_DIGEST_FIXTURE_PROVIDER": provider,
        "PYTHONPATH": os.pathsep.join((tempdir, src)),
    }
    return env, counts


def pdf_evidence(path):
    evidence = {
        "pdf_path": path or "",
        "pdf_bytes": 0,
        "pdf_sha256": "",
        "pdf_ok": False,
    }
    if not path or not os.path.isfile(path):
        return evidence
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        magic = handle.read(5)
        digest.update(magic)
        while True:
            chunk = handle.read(65536)
            if not chunk:
                break
            digest.update(chunk)
    evidence["pdf_bytes"] = os.path.getsize(path)
    evidence["pdf_sha256"] = digest.hexdigest()
    evidence["pdf_ok"] = magic.startswith(b"%PDF")
    return evidence


def newest_pdf(directory):
    if not directory or not os.path.isdir(directory):
        return ""
    candidates = [
        os.path.join(directory, name)
        for name in os.listdir(directory)
        if name.lower().endswith(".pdf")
    ]
    if not candidates:
        return ""
    return max(candidates, key=os.path.getmtime)


def mail_evidence(inbox_dir, since_ms, recipient):
    evidence = {
        "notifier_receipts": 0,
        "notifier_has_pdf": False,
        "notifier_subjects": [],
    }
    if not inbox_dir:
        return evidence
    index = os.path.join(inbox_dir, "inbox.jsonl")
    if not os.path.isfile(index):
        return evidence
    records = []
    with open(index, encoding="utf-8") as handle:
        for line in handle:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if int(record.get("received_at_ms", 0)) < since_ms:
                continue
            if recipient and recipient not in str(record.get("to", "")):
                continue
            records.append(record)
    evidence["notifier_receipts"] = len(records)
    evidence["notifier_has_pdf"] = any(r.get("has_pdf") for r in records)
    evidence["notifier_subjects"] = [r.get("subject", "") for r in records][-3:]
    return evidence


def parse_line(text, label):
    match = re.search(rf"^{re.escape(label)}:\s*(.*)$", text, re.MULTILINE)
    return match.group(1).strip() if match else ""


def failure_class(returncode, outcome, stdout, stderr, error_message):
    combined = (stdout or "") + "\n" + (stderr or "")
    if returncode is not None and returncode != 0:
        if "No LLM provider configured" in combined:
            return "no_llm_provider"
        if "Missing required environment variables" in combined:
            return "missing_github_env"
        if "paid model" in combined.lower():
            return "paid_model_forbidden"
        return "entrypoint_crash"
    if outcome != "success":
        if "GitHub API error" in error_message:
            return "github_error"
        if "Notification failed" in error_message:
            return "smtp_error"
        if "PDF render failed" in error_message:
            return "render_error"
        if "fail_exhausted" in combined or "No working model" in combined:
            return "no_free_model"
        return "target_error"
    return ""


def read_counts(path):
    counts = {
        "observed": False,
        "github_requests": None,
        "model_requests": None,
        "paid_model_attempts": None,
        "models": [],
    }
    if not path or not os.path.isfile(path):
        return counts
    try:
        with open(path, encoding="utf-8") as handle:
            raw = json.load(handle)
    except (OSError, ValueError):
        return counts
    counts.update({
        "observed": True,
        "github_requests": raw.get("github_requests"),
        "model_requests": raw.get("model_requests"),
        "paid_model_attempts": raw.get("paid_model_attempts"),
        "models": raw.get("models", []),
    })
    return counts


def write_metrics(metrics, path):
    if not path:
        return
    directory = os.path.dirname(path)
    if directory:
        os.makedirs(directory, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(metrics, handle, indent=2)
        handle.write("\n")


def main():
    from eval_trace import Tracer

    tracer = Tracer.from_env()
    mode = os.environ.get("EVAL_DIGEST_ENTRYPOINT_MODE", "preflight")
    if mode not in ("live", "fixture", "preflight"):
        raise SystemExit(f"unknown entrypoint mode: {mode}")
    bad_repo = os.environ.get("EVAL_DIGEST_BAD_REPO", "") in ("1", "true", "yes")
    src = os.environ.get("EVAL_DIGEST_SRC", "")
    if not src or not os.path.isdir(src):
        raise SystemExit("EVAL_DIGEST_SRC is required")
    provider = os.environ.get("EVAL_DIGEST_PROVIDER", "gemini").strip().lower()
    if provider not in PROVIDERS:
        raise SystemExit("only free Gemini or Groq providers are allowed")
    model = os.environ.get("EVAL_DIGEST_MODEL", "")
    allowed_models = PROVIDERS[provider]["models"] | {
        "mock-model", "free-fixture", "fixture/free-digest",
    }
    if model and model not in allowed_models and not model.endswith(":free"):
        raise SystemExit(f"paid model id is forbidden: {model!r}")
    if os.environ.get("EVAL_DIGEST_ALLOW_PAID", "") not in ("", "0"):
        raise SystemExit("EVAL_DIGEST_ALLOW_PAID must not be enabled")

    started_ms = int(time.time() * 1000)
    started_at = utc_now()
    tempdir = tempfile.mkdtemp(prefix="digest-entrypoint-")
    outdir = os.path.join(tempdir, "out")
    os.makedirs(outdir, exist_ok=True)
    metrics_out = os.environ.get("EVAL_DIGEST_METRICS_OUT", "")
    metrics_path = os.path.join(tempdir, "output-metrics.json")
    isolated_src = copy_entrypoint_src(src, tempdir)
    if mode == "preflight":
        provider_key, provider_source = "", "none"
    else:
        provider_key, provider_source = resolve_provider_key(provider)
    env = child_env(isolated_src, mode, bad_repo, provider, provider_key)
    env["DIGEST_OUTPUT_DIR"] = outdir
    counts_path = ""
    if mode == "fixture":
        fixture_env, counts_path = install_fixture_shim(
            tempdir, isolated_src, provider)
        env.update(fixture_env)

    skip_reason = ""
    if mode == "live":
        if not provider_key:
            skip_reason = "missing_credentials"
        if not os.environ.get("EVAL_DIGEST_GITHUB_TOKEN"):
            skip_reason = "missing_credentials"

    stdout = ""
    stderr = ""
    returncode = None
    with tracer.scope(
        "digest.entrypoint", mode=mode, bad_repo=bad_repo,
        provider=provider, free_only=True,
    ) as span:
        if skip_reason:
            outcome = "error"
            error_message = f"missing live credential: {skip_reason}"
            elapsed_ms = 0
        else:
            argv = [sys.executable, "-m", "digest_agent.main"]
            try:
                proc = subprocess.run(
                    argv, cwd=tempdir, env=env, capture_output=True,
                    text=True, timeout=TIMEOUT_S, check=False,
                )
                returncode = proc.returncode
                stdout = proc.stdout or ""
                stderr = proc.stderr or ""
            except subprocess.TimeoutExpired as error:
                returncode = None
                stdout = (error.stdout or "") if isinstance(error.stdout, str) else ""
                stderr = (error.stderr or "") if isinstance(error.stderr, str) else ""
            elapsed_ms = int((time.time() * 1000) - started_ms)
            outcome = parse_line(stdout, "Outcome") or (
                "timeout" if returncode is None else "error"
            )
            if (returncode is not None and returncode != 0
                    and "No LLM provider configured" not in (stdout + stderr)):
                outcome = "crash"
            error_message = parse_line(stdout, "Error")

        pdf_path = parse_line(stdout, "PDF") or newest_pdf(outdir)
        pdf = pdf_evidence(pdf_path)
        recipient = os.environ.get(
            "EVAL_DIGEST_RECIPIENT", "digest-e2e@test.invalid"
        )
        mail = mail_evidence(
            os.environ.get("EVAL_DIGEST_SMTP_INBOX", ""),
            started_ms, recipient,
        )
        email_field = parse_line(stdout, "Email sent")
        email_sent = (
            email_field.lower() == "true" if email_field
            else mail["notifier_receipts"] > 0
        )
        counts = read_counts(counts_path)
        if skip_reason:
            failure = skip_reason
        elif returncode is None:
            failure = "timeout"
        else:
            failure = failure_class(
                returncode, outcome, stdout, stderr, error_message
            )
        metrics = {
            "schema": "digest.output-metrics/v1",
            "entrypoint": "python -m digest_agent.main",
            "mode": mode,
            "provider": provider,
            "provider_key_source": provider_source,
            "free_only": True,
            "paid_model_configured": False,
            "run_id": os.environ.get("EVAL_TRACE_RUN_ID", ""),
            "trace_id": tracer.trace_id,
            "started_at": started_at,
            "finished_at": utc_now(),
            "wall_ms": elapsed_ms,
            "exit_status": returncode,
            "outcome": outcome,
            "summary": parse_line(stdout, "Summary"),
            "error_message": error_message,
            "failure_class": failure,
            "pdf_path": pdf["pdf_path"],
            "pdf_bytes": pdf["pdf_bytes"],
            "pdf_sha256": pdf["pdf_sha256"],
            "pdf_ok": pdf["pdf_ok"],
            "email_sent": email_sent,
            "notifier_receipts": mail["notifier_receipts"],
            "notifier_has_pdf": mail["notifier_has_pdf"],
            "notifier_subjects": mail["notifier_subjects"],
            "request_counts": counts,
            "inner_metrics": {
                "stages": "n/a",
                "tokens": "n/a",
                "cost": "n/a",
                "note": "target exposes no inner metrics; no proxy used",
            },
            "stdout_tail": stdout[-1000:],
            "stderr_tail": stderr[-1000:],
        }
        span.set(
            outcome=outcome, exit_status=returncode, failure_class=failure,
            pdf_ok=pdf["pdf_ok"], email_sent=email_sent,
            free_only=True,
        )

    write_metrics(metrics, metrics_path)
    if metrics_out:
        write_metrics(metrics, metrics_out)
    print(json.dumps(metrics))
    tracer.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
