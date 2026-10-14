#!/usr/bin/env python3
"""Offline driver for the real digest_agent orchestrator.

Wires the project's own fakes (FakeActivitySource, FakeLLMProvider,
FakeDocumentRenderer, FakeNotifier) into its real LangGraph
orchestrator and prints the final state as JSON on stdout.

Usage: driver.py (empty|single-pr) [--weak] [--fail-fetch] [--live]
       [--usage-out FILE] [--task-id ID]

- empty: no repository activity (empty-window case).
- single-pr: one repo with one merged PR (bounded-loop case).

With --usage-out, every model call is also recorded as an llm/v1
usage event (latency/outcome measured; tokens absent — the provider
port hides them). Events are labeled synthetic/benchmark.

No network, no keys, no PDF, no email. Runs entirely in-process.
"""

import json
import sys
import time

from digest_agent.core.fakes import (
    FakeActivitySource,
    FakeDocumentRenderer,
    FakeLLMProvider,
    FakeNotifier,
)
from digest_agent.core.orchestrator import build_orchestrator
from digest_agent.core.state import DigestState
from digest_agent.ports.activity_source import Commit, FileChange, PullRequest, RepoActivity
from usage import UsageReportingProvider


def build_activity(scenario):
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
    raise ValueError(f"unknown scenario: {scenario}")


class FailingActivitySource:
    """Activity source that always fails (upstream-outage scenario)."""

    def fetch_activity(self, repos, since):
        from digest_agent.ports.activity_source import ActivityFetchError

        raise ActivityFetchError("simulated upstream outage")


WEAK_COMPLETIONS = {
    "model-a": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
    "model-b": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
    "model-c": "SCORE: 2\nVERDICT: WEAK\nCRITIQUE: thin",
}


def flag(args, name):
    if name in args:
        index = args.index(name)
        if index + 1 < len(args):
            return args[index + 1]
    return None


def main():
    args = sys.argv[1:]
    scenario = args[0] if args else "empty"
    usage_out = flag(args, "--usage-out")
    task_id = flag(args, "--task-id") or f"{scenario}-{int(time.time() * 1000)}"
    weak = "--weak" in args
    fail_fetch = "--fail-fetch" in args
    live = "--live" in args
    if fail_fetch:
        activity_source = FailingActivitySource()
    else:
        activity_source = FakeActivitySource(activity=build_activity(scenario))
    if live:
        from live_provider import LivePortProvider

        provider = LivePortProvider()
    else:
        provider = FakeLLMProvider(
            completions=dict(WEAK_COMPLETIONS) if weak else None,
        )
    if usage_out and not live:
        provider = UsageReportingProvider(provider, usage_out, task_id)
    orchestrator = build_orchestrator(
        activity_source=activity_source,
        llm_provider=provider,
        document_renderer=FakeDocumentRenderer(),
        notifier=FakeNotifier(),
        max_attempts=3,
        max_reviews=3,
    )
    state = DigestState(repos=["owner/repo"], lookback_days=1)
    final = orchestrator.invoke(state)
    print(json.dumps({
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
    }))


if __name__ == "__main__":
    main()
