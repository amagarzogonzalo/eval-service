"""Real research runs for the eval-service integration.

Two modes share this runner:

``live``
    The target workflow with live model calls through eval proxy, while search
    and scrape stay pinned to the target's own cassettes. This is the bounded
    pre-E2E smoke path used by ``live_suite.yaml``.

``live_e2e``
    The target workflow with live search, live scrape/fetch and live model
    calls (through eval proxy). Retrieval is constrained by a scrape host
    allowlist and per-run call/source budgets from ``datasets/research_e2e``.
    Trace context from eval-service is propagated to the target's
    OpenTelemetry spans; keys and full page contents are never emitted.

Usage (executed by the target venv):
  python live_runner.py --repo TARGET --case research \
    --endpoint http://127.0.0.1:8796/v1 --model openai/gpt-4o-mini
  python live_runner.py --mode live_e2e --repo TARGET --topic rag-hallucinations \
    --dataset datasets/research_e2e/topics.json \
    --endpoint http://127.0.0.1:8796/v1 --model openai/gpt-4o-mini
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import shutil
import sys
import time
from pathlib import Path
from urllib.parse import urlparse


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--case", default="research")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--mode", choices=("live", "live_e2e"), default="live")
    parser.add_argument("--dataset", default="datasets/research_e2e/topics.json")
    parser.add_argument("--topic", default="rag-hallucinations")
    parser.add_argument("--max-sources", type=int, default=0)
    parser.add_argument("--max-scrapes", type=int, default=0)
    parser.add_argument("--traceparent", default="")
    parser.add_argument("--trace-endpoint", default="")
    return parser.parse_args()


def read_artifact(path: str | None) -> str:
    if not path:
        return ""
    artifact = Path(path)
    return artifact.read_text(encoding="utf-8") if artifact.exists() else ""


def load_topic(path: str, topic_id: str) -> dict[str, object]:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    for topic in data.get("topics", []):
        if topic.get("id") == topic_id:
            return topic
    raise ValueError(f"unknown topic: {topic_id}")


def host_allowed(host: str, allow_hosts: list[str]) -> bool:
    host = (host or "").lower().rstrip(".")
    for allowed in allow_hosts:
        allowed = allowed.lower().rstrip(".")
        if host == allowed or host.endswith("." + allowed):
            return True
    return False


class CountingSearch:
    """SearchPort proxy that counts calls and emits a retrieval span."""

    def __init__(self, inner: object, telemetry: object) -> None:
        self.inner = inner
        self.telemetry = telemetry
        self.name = getattr(inner, "name", "search")
        self.calls = 0

    async def search(self, query: str, *, limit: int = 10) -> object:
        self.calls += 1
        with self.telemetry.span(
            "eval.retrieval.search",
            **{"retrieval.query_chars": len(query), "retrieval.limit": limit},
        ) as current:
            records = await self.inner.search(query, limit=limit)
            current.set_attribute("retrieval.sources", len(records))
            return records

    async def fetch(self, doi: str) -> object:
        return await self.inner.fetch(doi)


class AllowlistScrape:
    """ScrapePort proxy enforcing a host allowlist and a fetch budget."""

    def __init__(
        self, inner: object, allow_hosts: list[str], max_scrapes: int, telemetry: object
    ) -> None:
        self.inner = inner
        self.allow_hosts = allow_hosts
        self.max_scrapes = max_scrapes
        self.telemetry = telemetry
        self.attempts = 0
        self.successes = 0
        self.blocked = 0
        self.hosts: set[str] = set()

    async def to_markdown(self, url: str) -> object:
        from knowledge_engine.core.exceptions import ScrapeError

        self.attempts += 1
        host = (urlparse(url).hostname or "").lower()
        if not host_allowed(host, self.allow_hosts):
            self.blocked += 1
            raise ScrapeError(f"host_not_allowlisted:{host}")
        if self.attempts > self.max_scrapes:
            raise ScrapeError("scrape_budget_exhausted")
        with self.telemetry.span(
            "eval.retrieval.scrape", **{"retrieval.url_host": host}
        ) as current:
            try:
                body = await self.inner.to_markdown(url)
            except Exception as exc:
                current.set_attribute("error.type", type(exc).__name__)
                raise
            current.set_attribute("retrieval.chars", len(getattr(body, "markdown", "")))
        self.successes += 1
        self.hosts.add(host)
        return body


class TracingProvider:
    """Provider proxy emitting one span per model call with usage, never content."""

    def __init__(self, inner: object, telemetry: object) -> None:
        self.inner = inner
        self.telemetry = telemetry
        self.name = getattr(inner, "name", "litellm")
        self.model_id = getattr(inner, "model_id", "")
        self.affinity = getattr(inner, "affinity", None)
        self.calls = 0
        self.input_tokens = 0
        self.output_tokens = 0

    def __getattr__(self, name: str) -> object:
        return getattr(self.inner, name)

    async def complete(self, req: object) -> object:
        self.calls += 1
        with self.telemetry.span(
            "llm.call",
            **{
                "gen_ai.system": self.name,
                "gen_ai.request.model": self.model_id,
            },
        ) as current:
            try:
                result = await self.inner.complete(req)
            except Exception as exc:
                current.set_attribute("error.type", type(exc).__name__)
                raise
            usage = getattr(result, "usage", None)
            if usage is not None:
                self.input_tokens += int(getattr(usage, "input_tokens", 0) or 0)
                self.output_tokens += int(getattr(usage, "output_tokens", 0) or 0)
                current.set_attribute("gen_ai.usage.input_tokens", usage.input_tokens)
                current.set_attribute("gen_ai.usage.output_tokens", usage.output_tokens)
            return result


def configure_telemetry(trace_endpoint: str) -> None:
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    if trace_endpoint:
        os.environ["OTEL_TRACES_EXPORTER"] = "otlp"
        os.environ["OTEL_EXPORTER_OTLP_ENDPOINT"] = trace_endpoint
    os.environ.setdefault("OTEL_SERVICE_NAME", "research-orchestration-agents")
    os.environ.setdefault("OTEL_METRICS_EXPORTER", "none")
    os.environ.setdefault("OTEL_BSP_SCHEDULE_DELAY", "500")


def attach_traceparent(traceparent: str) -> object:
    if not traceparent:
        return None
    from opentelemetry.context import attach
    from opentelemetry.propagate import extract

    token = attach(extract({"traceparent": traceparent}))
    return token


def force_flush() -> None:
    from opentelemetry import trace

    provider = trace.get_tracer_provider()
    flush = getattr(provider, "force_flush", None)
    if callable(flush):
        flush(timeout_millis=10000)


def trace_id_hex() -> str:
    from opentelemetry import trace

    context = trace.get_current_span().get_span_context()
    return f"{context.trace_id:032x}"


def count_citations(artifact: str) -> int:
    return len(set(re.findall(r"\[\^([^\]]+)\]", artifact)))


def build_search() -> object:
    from knowledge_engine.infrastructure.search.arxiv import ArxivAdapter
    from knowledge_engine.infrastructure.search.crossref import CrossrefAdapter
    from knowledge_engine.infrastructure.search.demux import DemuxSearchAdapter
    from knowledge_engine.infrastructure.search.europe_pmc import EuropePmcAdapter
    from knowledge_engine.infrastructure.search.openalex import OpenAlexAdapter
    from knowledge_engine.infrastructure.search.pubmed import PubMedAdapter
    from knowledge_engine.infrastructure.search.wikipedia import WikipediaAdapter

    return DemuxSearchAdapter(
        [
            OpenAlexAdapter(os.environ.get("OPENALEX_MAILTO", "")),
            CrossrefAdapter(os.environ.get("CROSSREF_MAILTO", "")),
            PubMedAdapter(),
            EuropePmcAdapter(),
            WikipediaAdapter(),
            ArxivAdapter(),
        ]
    )


def build_engine(repo: Path, root: Path, max_sources: int, scrape: object, provider: object) -> object:
    sys.path.insert(0, str(repo / "src"))
    sys.path.insert(0, str(repo))
    sys.path.insert(0, str(repo / "evals"))

    from knowledge_engine.engine import ResearchEngine
    from knowledge_engine.infrastructure.rate_limit import TokenBucketLimiter
    from knowledge_engine.infrastructure.router import FallbackRouter

    router = FallbackRouter([provider], limiter=TokenBucketLimiter())
    return ResearchEngine(
        store_root=str(root),
        search_limit=max_sources,
        search=build_search(),
        scrape=scrape,
        provider=router,
    )


def reset_store(root: Path, thread_id: str) -> None:
    shutil.rmtree(root / thread_id, ignore_errors=True)
    shutil.rmtree(root / "checkpoints" / thread_id, ignore_errors=True)
    shutil.rmtree(root / "wiki", ignore_errors=True)


def run_live(args: argparse.Namespace) -> dict[str, object]:
    repo = Path(args.repo).resolve()
    os.environ["OPENAI_API_BASE"] = args.endpoint
    os.environ["OPENAI_BASE_URL"] = args.endpoint
    os.environ["OPENAI_API_KEY"] = "dummy-eval-proxy"
    configure_telemetry(args.trace_endpoint)
    token = attach_traceparent(args.traceparent)

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
    from knowledge_engine.infrastructure.providers import LiteLLMProviderAdapter

    cassette = Cassette(f"{args.case}/run0", mode="replay")
    adapter = LiteLLMProviderAdapter(api_key="dummy-eval-proxy", cassette=None)
    adapter.name = "OpenRouter"
    adapter.model_id = args.model
    engine = build_engine(repo, root, max_sources, None, adapter)
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
            "trace_id": trace_id_hex(),
        }

    try:
        return asyncio.run(run())
    finally:
        force_flush()
        if token is not None:
            from opentelemetry.context import detach

            detach(token)


def run_live_e2e(args: argparse.Namespace) -> dict[str, object]:
    repo = Path(args.repo).resolve()
    topic = load_topic(args.dataset, args.topic)
    allow_hosts = [str(item) for item in topic.get("allow_hosts", [])]
    expected = dict(topic.get("expected") or {})
    max_sources = args.max_sources or int(topic.get("max_sources", 6))
    max_scrapes = args.max_scrapes or int(topic.get("max_scrapes", 6))

    os.environ["OPENAI_API_BASE"] = args.endpoint
    os.environ["OPENAI_BASE_URL"] = args.endpoint
    os.environ["OPENAI_API_KEY"] = "dummy-eval-proxy"
    os.environ.setdefault(
        "OTEL_RESOURCE_ATTRIBUTES",
        f"eval.topic={args.topic},deployment.environment=eval",
    )
    configure_telemetry(args.trace_endpoint)
    token = attach_traceparent(args.traceparent)

    from knowledge_engine.core.exceptions import ScrapeError
    from knowledge_engine.infrastructure.providers import LiteLLMProviderAdapter
    from knowledge_engine.infrastructure.scrape.scrape_router import ScrapeRouter
    from knowledge_engine.observability import telemetry

    root = Path("research_store/evals")
    scrape = AllowlistScrape(ScrapeRouter(), allow_hosts, max_scrapes, telemetry)
    adapter = LiteLLMProviderAdapter(api_key="dummy-eval-proxy", cassette=None)
    adapter.name = "OpenRouter"
    adapter.model_id = args.model
    provider = TracingProvider(adapter, telemetry)
    started = time.perf_counter()
    result: dict[str, object]

    try:
        with telemetry.span(
            "eval.research_e2e",
            **{
                "eval.topic": args.topic,
                "eval.kind": str(topic.get("kind", "research")),
                "gen_ai.request.model": args.model,
            },
        ) as current:
            if topic.get("kind") == "guardrail":
                engine = build_engine(repo, root, 1, scrape, provider)
                reset_store(root, "eval-guardrail")
                code = "ok"
                status = "fail"
                try:
                    asyncio.run(engine.scrape_run(str(topic["url"])))
                except ScrapeError as exc:
                    code = str(exc).split(":", 1)[0]
                    status = "blocked" if code == "host_not_allowlisted" else "fail"
                result = {
                    "topic": args.topic,
                    "kind": "guardrail",
                    "status": status,
                    "code": code,
                    "failures": [] if status == "blocked" else [f"guardrail:{code}"],
                    "retrieval": {
                        "mode": "live",
                        "network_calls": 0,
                        "search_calls": 0,
                        "scrape_attempts": scrape.attempts,
                        "scrape_calls": scrape.successes,
                        "blocked": scrape.blocked,
                        "hosts": sorted(scrape.hosts),
                    },
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                    "trace_id": trace_id_hex(),
                    "trace_exported": bool(args.trace_endpoint),
                }
            else:
                search = CountingSearch(build_search(), telemetry)
                engine = build_engine(repo, root, max_sources, scrape, provider)
                engine._search = search
                reset_store(root, "eval-e2e-research")
                run_result = asyncio.run(
                    engine.research(
                        str(topic["query"]),
                        workflow="research",
                        thread_id="eval-e2e-research",
                    )
                )
                state = run_result.state
                artifact = read_artifact(state.output_path)
                sources = state.sources or []
                bodies = list(state.bodies or [])
                artifact_sha = hashlib.sha256(artifact.encode("utf-8")).hexdigest()
                current.set_attribute("artifact.chars", len(artifact))
                current.set_attribute("artifact.sha256", artifact_sha)

                import yaml

                target_case = yaml.safe_load(
                    (repo / "evals" / "cases" / "research.yaml").read_text(encoding="utf-8")
                )
                failures = list(
                    invariants_run(artifact, target_case.get("invariants", {}), sources)
                )
                usage = {
                    "input_tokens": provider.input_tokens,
                    "output_tokens": provider.output_tokens,
                    "total_tokens": provider.input_tokens + provider.output_tokens,
                    "model_calls": provider.calls,
                }
                sources_count = len(sources)
                citations = count_citations(artifact)
                if sources_count < int(expected.get("min_sources", 1)):
                    failures.append(f"expected_sources: {sources_count}")
                if len(bodies) < int(expected.get("min_bodies", 1)):
                    failures.append(f"expected_bodies: {len(bodies)}")
                if len(artifact) < int(expected.get("min_artifact_chars", 1)):
                    failures.append(f"expected_artifact_chars: {len(artifact)}")
                if citations < int(expected.get("min_citations", 1)):
                    failures.append(f"expected_citations: {citations}")
                if usage["total_tokens"] > int(expected.get("max_total_tokens", 30000)):
                    failures.append(f"token_budget: {usage['total_tokens']}")
                if usage["model_calls"] > int(expected.get("max_model_calls", 24)):
                    failures.append(f"model_call_budget: {usage['model_calls']}")
                if scrape.successes < 1:
                    failures.append("expected_live_scrape: 0")
                if search.calls < 1:
                    failures.append("expected_live_search: 0")
                lowered = artifact.lower()
                for pattern in expected.get("forbidden_patterns", []):
                    if re.search(str(pattern), lowered):
                        failures.append(f"forbidden_pattern: {pattern}")

                result = {
                    "topic": args.topic,
                    "kind": "research",
                    "query": str(topic["query"]),
                    "status": "pass" if not failures else "fail",
                    "ok": not failures,
                    "failures": failures,
                    "model": args.model,
                    "artifact_chars": len(artifact),
                    "artifact_sha256": artifact_sha,
                    "citations": citations,
                    "nsources": sources_count,
                    "bodies": len(bodies),
                    "usage": usage,
                    "retrieval": {
                        "mode": "live",
                        "network_calls": search.calls + scrape.attempts,
                        "search_calls": search.calls,
                        "scrape_attempts": scrape.attempts,
                        "scrape_calls": scrape.successes,
                        "blocked": scrape.blocked,
                        "hosts": sorted(scrape.hosts),
                    },
                    "elapsed_ms": int((time.perf_counter() - started) * 1000),
                    "trace_id": trace_id_hex(),
                    "trace_exported": bool(args.trace_endpoint),
                }

            failures = list(result.get("failures") or [])
            with telemetry.span(
                "eval.grade",
                **{
                    "grade.pass": not failures,
                    "grade.failures": len(failures),
                },
            ) as grade:
                grade.set_attribute("grade.pass", not failures)
                grade.set_attribute("grade.failures", len(failures))
                grade.set_attribute("artifact.chars", int(result.get("artifact_chars", 0)))
    finally:
        force_flush()
        if token is not None:
            from opentelemetry.context import detach

            detach(token)
    return result


def invariants_run(artifact: str, spec: dict[str, object], sources: object) -> list[str]:
    from evals.lib import invariants

    return invariants.run(artifact, spec, sources)


def main() -> int:
    args = parse_args()
    repo = Path(args.repo).resolve()
    sys.path.insert(0, str(repo / "src"))
    sys.path.insert(0, str(repo))
    sys.path.insert(0, str(repo / "evals"))

    if args.mode == "live_e2e":
        result = run_live_e2e(args)
    else:
        result = run_live(args)
    print(json.dumps(result, ensure_ascii=False, default=str))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
