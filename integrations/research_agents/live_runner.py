#!/usr/bin/env python3
"""Real research run with live LLMs through the eval proxy.

The target's search and scrape calls are replayed from its own cassettes so the
run is bounded; every model call goes to an OpenAI-compatible loopback endpoint
(the eval proxy), which is the single observer and key holder.

Usage (executed by the target venv):
  python live_runner.py --repo TARGET --case research \
    --endpoint http://127.0.0.1:8795/v1 --model openai/gpt-4o-mini
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--case", default="research")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--model", required=True)
    return parser.parse_args()


def read_artifact(path: str | None) -> str:
    if not path:
        return ""
    artifact = Path(path)
    return artifact.read_text(encoding="utf-8") if artifact.exists() else ""


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).resolve()
    sys.path.insert(0, str(repo / "src"))
    sys.path.insert(0, str(repo))
    sys.path.insert(0, str(repo / "evals"))

    os.environ["OPENAI_API_BASE"] = args.endpoint
    os.environ["OPENAI_BASE_URL"] = args.endpoint
    os.environ["OPENAI_API_KEY"] = "dummy-eval-proxy"
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"

    import yaml

    case = yaml.safe_load(
        (repo / "evals" / "cases" / f"{args.case}.yaml").read_text(encoding="utf-8")
    )
    query = str(case["input"]["query"])
    max_sources = int(case["input"].get("max_sources", 8))

    root = Path("research_store/evals")
    shutil.rmtree(root / "eval-live-research", ignore_errors=True)
    shutil.rmtree(root / "checkpoints" / "eval-live-research", ignore_errors=True)
    shutil.rmtree(root / "wiki", ignore_errors=True)

    from evals.adapters import _RecordingScrape, _RecordingSearch
    from evals.lib import invariants
    from evals.lib.cassette import Cassette
    from knowledge_engine.engine import ResearchEngine
    from knowledge_engine.infrastructure.providers import LiteLLMProviderAdapter
    from knowledge_engine.infrastructure.rate_limit import TokenBucketLimiter
    from knowledge_engine.infrastructure.router import FallbackRouter

    cassette = Cassette(f"{args.case}/run0", mode="replay")
    adapter = LiteLLMProviderAdapter(api_key="dummy-eval-proxy", cassette=None)
    adapter.name = "OpenRouter"
    adapter.model_id = f"openai/{args.model}"
    router = FallbackRouter([adapter], limiter=TokenBucketLimiter())
    engine = ResearchEngine(
        store_root=str(root),
        search_limit=max_sources,
        provider=router,
    )
    # Retrieval stays pinned to the target's recordings; only model calls live.
    engine._search = _RecordingSearch(engine._search, cassette)
    engine._scrape = _RecordingScrape(engine._scrape, cassette)

    async def run() -> dict[str, object]:
        result = await engine.research(
            query,
            workflow="research",
            thread_id="eval-live-research",
        )
        state = result.state
        artifact = read_artifact(state.output_path)
        sources = state.sources or []
        failures = invariants.run(artifact, case.get("invariants", {}), sources)
        return {
            "case": args.case,
            "ok": not failures,
            "status": "pass" if not failures else "fail",
            "failures": failures,
            "artifact_chars": len(artifact),
            "artifact_sha256": hashlib.sha256(artifact.encode("utf-8")).hexdigest(),
            "nsources": len(sources),
            "bodies": len(state.bodies or []),
            "model": args.model,
            "workflow": "research",
        }

    print(json.dumps(asyncio.run(run()), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
