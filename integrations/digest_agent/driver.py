#!/usr/bin/env python3
"""Offline driver for the real digest_agent orchestrator.

Wires the project's own fakes (FakeActivitySource, FakeLLMProvider,
FakeDocumentRenderer, FakeNotifier) into its real LangGraph
orchestrator and prints the final state as JSON on stdout.

Usage: driver.py (empty|single-pr)

- empty: no repository activity (empty-window case).
- single-pr: one repo with one merged PR (bounded-loop case).

No network, no keys, no PDF, no email. Runs entirely in-process.
"""

import json
import sys

from digest_agent.core.fakes import (
    FakeActivitySource,
    FakeDocumentRenderer,
    FakeLLMProvider,
    FakeNotifier,
)
from digest_agent.core.orchestrator import build_orchestrator
from digest_agent.core.state import DigestState
from digest_agent.ports.activity_source import Commit, FileChange, PullRequest, RepoActivity


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


def main():
    scenario = sys.argv[1] if len(sys.argv) > 1 else "empty"
    activity = build_activity(scenario)
    orchestrator = build_orchestrator(
        activity_source=FakeActivitySource(activity=activity),
        llm_provider=FakeLLMProvider(),
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
    }))


if __name__ == "__main__":
    main()
