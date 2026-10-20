#!/usr/bin/env python3
"""Driver for the real digest_agent orchestrator.

Offline mode wires the project's own fakes (FakeActivitySource,
FakeLLMProvider, FakeDocumentRenderer, FakeNotifier) into its real
LangGraph orchestrator and prints the final state as JSON on stdout.

E2E mode (``--real-services``) swaps in real adapters from the target
repository and this integration, without editing the target:

- activity: real ``GitHubAdapter`` against api.github.com, or a recorded
  GitHub fixture (explicitly labeled);
- LLM: ``live_provider.LivePortProvider`` through eval-proxy (or any
  OpenAI-compatible endpoint);
- renderer: the target's real ``ReportLabAdapter``;
- notifier: the target's real ``SMTPAdapter`` (local test inbox or an
  operator-supplied SMTP host) or ``ResendAdapter``.

Every real service call is wrapped in a trace span (see ``trace.py``).
Side effects are verified after the run: the PDF is hashed and checked
for a ``%PDF`` header, and the SMTP inbox index is queried for the
message that this run just produced.

Usage: driver.py (empty|single-pr|github|github-fixture|bad-repo)
       [--weak] [--fail-fetch] [--live] [--real-services]
       [--usage-out FILE] [--task-id ID]
"""

import hashlib
import json
import os
import sys
import tempfile
import time
from urllib.parse import urlparse

from digest_agent.core.fakes import (
    FakeActivitySource,
    FakeDocumentRenderer,
    FakeLLMProvider,
    FakeNotifier,
)
from digest_agent.core.orchestrator import build_orchestrator
from digest_agent.core.state import DigestState
from digest_agent.ports.activity_source import (
    Commit,
    FileChange,
    PullRequest,
    RepoActivity,
)
from eval_trace import Tracer
from usage import UsageReportingProvider

SCENARIOS = ("empty", "single-pr", "github", "github-fixture", "bad-repo")
DEFAULT_REPO = "amagarzogonzalo/eval-service"
DEFAULT_BAD_REPO = "amagarzogonzalo/eval-service-e2e-missing-repo"
DEFAULT_RECIPIENT = "digest-e2e@test.invalid"


def log(message):
    sys.stderr.write(f"digest-driver: {message}\n")
    sys.stderr.flush()


def flag(args, name):
    if name in args:
        index = args.index(name)
        if index + 1 < len(args):
            return args[index + 1]
    return None


def env_list(name, default):
    raw = os.environ.get(name, default)
    return [item.strip() for item in raw.split(",") if item.strip()]


# --------------------------------------------------------------------------
# Activity sources
# --------------------------------------------------------------------------

def build_fake_activity(scenario):
    if scenario == "empty":
        return []
    if scenario == "single-pr":
        change = FileChange(
            filename="src/app.py", status="modified",
            additions=10, deletions=2, patch="@@ -1,2 +1,10 @@\n+line\n",
        )
        commit = Commit(sha="abc1234", message="Add feature", author="dev", files=[change])
        pr = PullRequest(
            number=7, title="Add feature", state="merged", merged=True,
            files=[change], additions=10, deletions=2, changed_files=1,
        )
        return [RepoActivity(repo="owner/repo", commits=[commit], pull_requests=[pr])]
    raise ValueError(f"unknown fake scenario: {scenario}")


class FailingActivitySource:
    """Activity source that always fails (upstream-outage scenario)."""

    def fetch_activity(self, repos, since):
        from digest_agent.ports.activity_source import ActivityFetchError

        raise ActivityFetchError("simulated upstream outage")


def build_fixture_source(path):
    with open(path, encoding="utf-8") as handle:
        payload = json.load(handle)
    repos = payload.get("repos", payload) if isinstance(payload, dict) else payload
    activities = []
    for item in repos:
        commits = []
        for commit in item.get("commits", []):
            commits.append(Commit(
                sha=commit["sha"],
                message=commit["message"],
                author=commit.get("author", ""),
                files=[FileChange(**f) for f in commit.get("files", [])],
            ))
        pull_requests = []
        for pr in item.get("pull_requests", []):
            pull_requests.append(PullRequest(
                number=pr["number"],
                title=pr["title"],
                state=pr.get("state", "closed"),
                merged=bool(pr.get("merged")),
                files=[FileChange(**f) for f in pr.get("files", [])],
                additions=int(pr.get("additions", 0) or 0),
                deletions=int(pr.get("deletions", 0) or 0),
                changed_files=int(pr.get("changed_files", 0) or 0),
            ))
        activities.append(RepoActivity(
            repo=item["repo"], commits=commits, pull_requests=pull_requests,
        ))
    return activities


class FixtureActivitySource:
    name = "github-fixture"

    def __init__(self, path):
        self.path = path
        self.activities = build_fixture_source(path)

    def fetch_activity(self, repos, since):
        return list(self.activities)


def build_github_source(repos, scenario):
    token = os.environ.get("EVAL_DIGEST_GITHUB_TOKEN", "")
    if not token:
        raise SystemExit(
            "EVAL_DIGEST_GITHUB_TOKEN is required for real GitHub activity "
            "(read-only test token from SSM; never logged)"
        )
    from digest_agent.adapters.driven.github_adapter import GitHubAdapter

    lookback = int(os.environ.get("EVAL_DIGEST_LOOKBACK_DAYS", "7"))
    source_repos = ",".join(repos)
    return GitHubAdapter(token=token, repos=source_repos, lookback_days=lookback)


def build_activity_source(scenario, activity_name, repos):
    if activity_name == "fake":
        return FakeActivitySource(activity=build_fake_activity(scenario)), repos
    if activity_name == "failing":
        return FailingActivitySource(), repos
    if activity_name == "github":
        source_repos = repos if scenario != "bad-repo" else env_list(
            "EVAL_DIGEST_BAD_REPOS", DEFAULT_BAD_REPO)
        return build_github_source(source_repos, scenario), source_repos
    if activity_name == "fixture":
        fixture = os.environ.get(
            "EVAL_DIGEST_ACTIVITY_FIXTURE",
            "datasets/digest_e2e/github_activity.json",
        )
        return FixtureActivitySource(os.path.expandvars(fixture)), repos
    raise SystemExit(f"unknown EVAL_DIGEST_ACTIVITY: {activity_name!r}")


# --------------------------------------------------------------------------
# Trace-wrapped service adapters
# --------------------------------------------------------------------------

class TracedActivitySource:
    def __init__(self, inner, tracer, root, name):
        self.inner = inner
        self.tracer = tracer
        self.root = root
        self.name = name

    def fetch_activity(self, repos, since):
        with self.tracer.scope(
            "digest.activity.fetch", parent=self.root,
            source=self.name, repo_count=len(repos),
        ) as span:
            result = self.inner.fetch_activity(repos, since)
            prs = sum(len(r.pull_requests) for r in result)
            commits = sum(len(r.commits) for r in result)
            files = sum(
                len(pr.files) for r in result for pr in r.pull_requests
            )
            span.set(
                repos_returned=len(result), pull_requests=prs,
                commits=commits, files=files,
            )
            return result


class TracedProvider:
    def __init__(self, inner, tracer, root):
        self.inner = inner
        self.tracer = tracer
        self.root = root

    def _link(self, span):
        linker = getattr(self.inner, "set_traceparent", None)
        if callable(linker):
            linker(span.traceparent())

    def list_free_models(self):
        with self.tracer.scope("digest.llm.list", parent=self.root) as span:
            models = self.inner.list_free_models()
            span.set(count=len(models), models=",".join(str(m) for m in models)[:300])
            return models

    def test(self, model_id):
        with self.tracer.scope(
            "digest.llm.test", parent=self.root, model=str(model_id),
        ) as span:
            self._link(span)
            result = self.inner.test(model_id)
            span.set(passed=bool(result.passed), reason=str(result.reason)[:120])
            return result

    def complete(self, model_id, prompt):
        with self.tracer.scope(
            "digest.llm.complete", parent=self.root,
            model=str(model_id), prompt_chars=len(prompt),
        ) as span:
            self._link(span)
            text = self.inner.complete(model_id, prompt)
            span.set(response_chars=len(text))
            return text


class TracedRenderer:
    def __init__(self, inner, tracer, root, name):
        self.inner = inner
        self.tracer = tracer
        self.root = root
        self.name = name

    def render(self, summary, extra_meta=None):
        with self.tracer.scope(
            "digest.render", parent=self.root, renderer=self.name,
            summary_chars=len(summary),
        ) as span:
            try:
                path = self.inner.render(summary, extra_meta=extra_meta)
            except TypeError:
                path = self.inner.render(summary)
            evidence = pdf_evidence(path)
            span.set(
                output_bytes=evidence["pdf_bytes"],
                sha256=evidence["pdf_sha256"],
                is_pdf=evidence["pdf_ok"],
            )
            return path


class TracedNotifier:
    def __init__(self, inner, tracer, root, name):
        self.inner = inner
        self.tracer = tracer
        self.root = root
        self.name = name

    def send(self, recipient, subject, attachment_path):
        domain = recipient.split("@")[-1] if "@" in recipient else "unknown"
        attachment_bytes = (
            os.path.getsize(attachment_path)
            if attachment_path and os.path.isfile(attachment_path) else 0
        )
        with self.tracer.scope(
            "digest.notify", parent=self.root, notifier=self.name,
            recipient_domain=domain, attachment_bytes=attachment_bytes,
        ) as span:
            self.inner.send(recipient, subject, attachment_path)
            span.set(delivered=True)


# --------------------------------------------------------------------------
# Evidence
# --------------------------------------------------------------------------

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


def mail_evidence(inbox_dir, since_ms, recipient):
    evidence = {
        "notifier_receipts": 0,
        "notifier_has_pdf": False,
        "notifier_tls": False,
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
    evidence["notifier_tls"] = any(r.get("tls") for r in records)
    evidence["notifier_subjects"] = [r.get("subject", "") for r in records][-3:]
    return evidence


# --------------------------------------------------------------------------
# Service wiring
# --------------------------------------------------------------------------

def build_renderer(name):
    if name == "fake":
        return FakeDocumentRenderer()
    if name == "reportlab":
        output_dir = os.environ.get(
            "EVAL_DIGEST_OUTPUT_DIR",
            os.path.join(tempfile.gettempdir(), "digest-e2e"),
        )
        os.makedirs(output_dir, exist_ok=True)
        os.environ["DIGEST_OUTPUT_DIR"] = output_dir
        from digest_agent.adapters.driven.reportlab_adapter import ReportLabAdapter

        return ReportLabAdapter()
    raise SystemExit(f"unknown EVAL_DIGEST_RENDERER: {name!r}")


def build_notifier(name):
    if name == "fake":
        return FakeNotifier()
    if name == "smtp":
        if not os.environ.get("SMTP_HOST"):
            raise SystemExit(
                "EVAL_DIGEST_NOTIFIER=smtp requires SMTP_HOST (the adapter "
                "starts a local test sink and sets it; external SMTP must "
                "set SMTP_* explicitly)"
            )
        from digest_agent.adapters.driven.gmail_adapter import SMTPAdapter

        return SMTPAdapter()
    if name == "resend":
        key = os.environ.get("EVAL_DIGEST_RESEND_API_KEY", "")
        if key:
            os.environ["RESEND_API_KEY"] = key
        sender = os.environ.get("EVAL_DIGEST_RESEND_FROM", "")
        if sender:
            os.environ["RESEND_FROM"] = sender
        from digest_agent.adapters.driven.resend_adapter import ResendAdapter

        return ResendAdapter()
    raise SystemExit(f"unknown EVAL_DIGEST_NOTIFIER: {name!r}")


def activity_name_for(scenario):
    explicit = os.environ.get("EVAL_DIGEST_ACTIVITY", "")
    if explicit:
        return explicit
    if scenario == "github":
        return "github"
    if scenario == "github-fixture":
        return "fixture"
    if scenario == "bad-repo":
        return "github"
    return "fake"


def main():
    args = sys.argv[1:]
    scenario = args[0] if args else "empty"
    if scenario not in SCENARIOS:
        raise SystemExit("unknown scenario: {} (expected one of {})".format(
            scenario, ", ".join(SCENARIOS)))
    usage_out = flag(args, "--usage-out")
    task_id = flag(args, "--task-id") or f"{scenario}-{int(time.time() * 1000)}"
    weak = "--weak" in args
    fail_fetch = "--fail-fetch" in args
    live = "--live" in args
    real_services = "--real-services" in args or os.environ.get("EVAL_DIGEST_E2E") == "1"

    activity_name = activity_name_for(scenario)
    renderer_name = os.environ.get(
        "EVAL_DIGEST_RENDERER", "reportlab" if real_services else "fake")
    notifier_name = os.environ.get(
        "EVAL_DIGEST_NOTIFIER", "smtp" if real_services else "fake")
    if real_services:
        missing = [
            name for name, value in (
                ("activity", activity_name),
                ("renderer", renderer_name),
                ("notifier", notifier_name),
            ) if value in ("fake", "failing")
        ]
        if missing:
            raise SystemExit(
                "real-services mode forbids fake {}; configure the real "
                "adapter instead of silently faking it".format(", ".join(missing))
            )

    tracer = Tracer.from_env()
    started_ms = int(time.time() * 1000)
    repos = (
        env_list("EVAL_DIGEST_GITHUB_REPOS", DEFAULT_REPO)
        if activity_name in ("github", "fixture")
        else ["owner/repo"]
    )
    lookback = int(os.environ.get("EVAL_DIGEST_LOOKBACK_DAYS", "7"))
    recipient = os.environ.get("EVAL_DIGEST_RECIPIENT", DEFAULT_RECIPIENT)
    inbox_dir = os.environ.get("EVAL_DIGEST_SMTP_INBOX", "")

    with tracer.scope(
        "digest.run", scenario=scenario, live=live,
        activity=activity_name, renderer=renderer_name,
        notifier=notifier_name, repo_count=len(repos),
        model=os.environ.get("EVAL_DIGEST_MODEL", ""),
        llm_host=_endpoint_host() if live else "",
    ) as run_span:
        source, state_repos = build_activity_source(
            scenario, activity_name, repos)
        source = TracedActivitySource(
            source, tracer, run_span, activity_name)

        if fail_fetch:
            source = TracedActivitySource(
                FailingActivitySource(), tracer, run_span, "failing")

        if live:
            from live_provider import LivePortProvider

            os.environ["EVAL_DIGEST_PROXY_TRACEPARENT"] = run_span.traceparent()
            provider = LivePortProvider()
            usage_origin = "proxy"
        else:
            provider = FakeLLMProvider(
                completions=dict(WEAK_COMPLETIONS) if weak else None,
            )
            usage_origin = "synthetic"
            if usage_out:
                provider = UsageReportingProvider(provider, usage_out, task_id)
        provider = TracedProvider(provider, tracer, run_span)

        renderer = TracedRenderer(
            build_renderer(renderer_name), tracer, run_span, renderer_name)
        notifier = TracedNotifier(
            build_notifier(notifier_name), tracer, run_span, notifier_name)

        orchestrator = build_orchestrator(
            activity_source=source,
            llm_provider=provider,
            document_renderer=renderer,
            notifier=notifier,
            max_attempts=3,
            max_reviews=3,
        )
        state = DigestState(
            repos=state_repos,
            lookback_days=lookback,
            recipient=recipient,
        )
        final = orchestrator.invoke(state)

        pdf = pdf_evidence(final.pdf_path)
        mail = mail_evidence(inbox_dir, started_ms, recipient)
        activity = final.activity or []
        result = {
            "scenario": scenario,
            "summary": final.summary or "",
            "deterministic_summary": final.deterministic_summary or "",
            "attempt_count": final.attempt_count,
            "selected_model": final.selected_model,
            "email_sent": final.email_sent,
            "pdf_path": final.pdf_path or "",
            "outcome": final.outcome or "",
            "error_message": final.error_message or "",
            "review_count": final.review_count,
            "review_decision": final.review_decision or "",
            "review_score": final.review_score,
            "activity_source": activity_name,
            "activity_repos": [r.repo for r in activity],
            "activity_prs": sum(len(r.pull_requests) for r in activity),
            "activity_commits": sum(len(r.commits) for r in activity),
            "activity_files": sum(
                len(pr.files) for r in activity for pr in r.pull_requests
            ),
            "renderer": renderer_name,
            "notifier": notifier_name,
            "live": live,
            "usage_origin": usage_origin,
            "trace_id": tracer.trace_id,
            "trace_span_id": run_span.span_id,
            "trace_out": tracer.out_path,
        }
        result.update(pdf)
        result.update(mail)
        run_span.set(
            outcome=result["outcome"],
            email_sent=bool(result["email_sent"]),
            notifier_receipts=result["notifier_receipts"],
            pdf_bytes=result["pdf_bytes"],
            activity_prs=result["activity_prs"],
        )
        with tracer.scope(
            "digest.result", parent=run_span,
            outcome=result["outcome"],
            email_sent=bool(result["email_sent"]),
            notifier_receipts=result["notifier_receipts"],
            pdf_bytes=result["pdf_bytes"],
        ):
            pass

    print(json.dumps(result))
    tracer.flush()


def _endpoint_host():
    endpoint = os.environ.get("EVAL_DIGEST_ENDPOINT", "https://api.openai.com/v1")
    try:
        parsed = urlparse(endpoint)
        return parsed.hostname or ""
    except ValueError:
        return ""


WEAK_COMPLETIONS = {
    "model-a": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
    "model-b": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
    "model-c": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
}


if __name__ == "__main__":
    main()
