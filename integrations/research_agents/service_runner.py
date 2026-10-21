"""Black-box research service runner for eval-service.

One HTTP client for two endpoints:

- ``local``  a target FastAPI app started by ``run_live_service.sh``
- ``remote`` the deployed Render service

The client never imports target code. It exercises the public contract:
``/health``, ``/readiness``, ``POST /research``, ``/result/{task_id}``,
``/result/{task_id}/artifact`` and ``/result/{task_id}/sources``.

Evidence is written by the eval run store / S3. This runner never queries
Supabase. Tokens are read from the environment and are never printed or
included in the emitted JSON.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

DEFAULT_TIMEOUT_S = 1200
DEFAULT_POLL_S = 2.0
DEFAULT_POLL_MAX_S = 10.0
DEFAULT_HTTP_S = 120.0
DEFAULT_DATASET = "datasets/research_e2e/topics.json"
DEFAULT_TOPIC = "rag-hallucinations"
TERMINAL = {"done", "error"}
RUNNING = {"running"}
QUEUED = {"queued"}


class ServiceError(RuntimeError):
    """A classified service failure."""

    def __init__(self, code: str, message: str, *, status: int | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.status = status


def now_ms() -> int:
    return int(time.time() * 1000)


def load_topic(path: str, topic_id: str) -> dict:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    for topic in data.get("topics", []):
        if topic.get("id") == topic_id:
            return topic
    raise ServiceError("unknown_topic", f"topic not found: {topic_id}")


def service_token() -> str:
    """Read the bearer token without exposing it in argv or output."""
    for name in ("EVAL_RESEARCH_SERVICE_TOKEN", "RENDER_API_KEY"):
        value = os.environ.get(name, "").strip()
        if value:
            return value
    return ""


def redact(message: str, token: str) -> str:
    if token:
        message = message.replace(token, "<redacted>")
    return message[:800]


def request(
    base_url: str,
    path: str,
    *,
    token: str = "",
    method: str = "GET",
    payload: dict | None = None,
    timeout_s: float = DEFAULT_HTTP_S,
    traceparent: str = "",
    headers: dict[str, str] | None = None,
    want_json: bool = True,
) -> dict:
    url = f"{base_url.rstrip('/')}{path}"
    body = None
    request_headers = {
        "Accept": "application/json" if want_json else "text/markdown",
        "Connection": "close",
    }
    if payload is not None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        request_headers["Content-Type"] = "application/json"
    if token:
        request_headers["Authorization"] = f"Bearer {token}"
    if traceparent:
        request_headers["traceparent"] = traceparent
        request_headers["X-Eval-Trace-Id"] = traceparent.split("-")[1]
    if headers:
        request_headers.update(headers)
    req = urllib.request.Request(url, data=body, headers=request_headers, method=method)
    start = time.perf_counter()
    status = 0
    raw = b""
    response_headers: dict[str, str] = {}
    attempts = 3 if method == "GET" else 1
    for attempt in range(attempts):
        try:
            with urllib.request.urlopen(req, timeout=timeout_s) as resp:
                status = resp.status
                response_headers = dict(resp.headers.items())
                raw = resp.read()
            break
        except urllib.error.HTTPError as exc:
            status = exc.code
            response_headers = dict(exc.headers.items()) if exc.headers else {}
            raw = exc.read()
            break
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            if attempt + 1 < attempts:
                time.sleep(0.1 * (attempt + 1))
                continue
            raise ServiceError("service_unreachable", redact(str(exc), token)) from exc
    elapsed_ms = int((time.perf_counter() - start) * 1000)
    text = raw.decode("utf-8", errors="replace")
    parsed = None
    if want_json and text:
        try:
            parsed = json.loads(text)
        except ValueError:
            parsed = None
    return {
        "status": status,
        "text": text,
        "json": parsed,
        "bytes": len(raw),
        "latency_ms": elapsed_ms,
        "headers": response_headers,
    }


def retry_after_s(headers: dict[str, str], fallback: float) -> float:
    raw = headers.get("Retry-After") or headers.get("retry-after") or ""
    try:
        value = float(raw)
    except ValueError:
        return fallback
    return max(0.0, min(value, 60.0))


def health(base_url: str, timeout_s: float, traceparent: str) -> dict:
    result = request(base_url, "/health", timeout_s=timeout_s, traceparent=traceparent)
    body = result["json"] or {}
    ok = result["status"] == 200 and body.get("status") == "ok"
    return {
        "status": result["status"],
        "ms": result["latency_ms"],
        "ok": ok,
        "engine": body.get("engine"),
    }


def readiness(base_url: str, timeout_s: float, traceparent: str) -> dict:
    result = request(base_url, "/readiness", timeout_s=timeout_s, traceparent=traceparent)
    body = result["json"] or {}
    checks = body.get("checks") or {}
    return {
        "status": result["status"],
        "ms": result["latency_ms"],
        "ok": bool(body.get("ok")),
        "checks": checks,
    }


def effective_ready(lane: str, health_result: dict, readiness_result: dict) -> bool:
    if not health_result.get("ok"):
        return False
    if lane == "remote":
        return bool(readiness_result.get("ok"))
    # Local stores are expected to fail DB/storage probes; providers/auth are
    # the checks that actually decide whether a run can serve.
    checks = readiness_result.get("checks") or {}
    return bool(checks.get("providers")) and bool(checks.get("auth"))


def submit(
    base_url: str,
    token: str,
    query: str,
    thread_id: str,
    timeout_s: float,
    traceparent: str,
) -> dict:
    payload = {"query": query, "workflow": "research", "thread_id": thread_id}
    deadline = time.monotonic() + timeout_s
    last_error = None
    for attempt in range(3):
        result = request(
            base_url,
            "/research",
            token=token,
            method="POST",
            payload=payload,
            timeout_s=min(timeout_s, DEFAULT_HTTP_S),
            traceparent=traceparent,
        )
        if result["status"] in (200, 202):
            body = result["json"] or {}
            task_id = str(body.get("task_id") or "")
            if not task_id:
                raise ServiceError("service_protocol", "POST /research returned no task_id")
            return {
                "status": result["status"],
                "ms": result["latency_ms"],
                "task_id": task_id,
                "task_status": body.get("status"),
                "attempts": attempt + 1,
            }
        if result["status"] in (401, 403):
            raise ServiceError(
                "service_auth",
                f"service rejected bearer token (HTTP {result['status']})",
                status=result["status"],
            )
        if result["status"] == 429:
            delay = retry_after_s(result["headers"], 10.0)
            last_error = ServiceError(
                "service_queue_full",
                f"service queue/caller limit reached (HTTP 429); retry after {delay:.1f}s",
                status=429,
            )
            if attempt < 2 and time.monotonic() + delay < deadline:
                time.sleep(delay)
                continue
            raise last_error
        if 500 <= result["status"] < 600:
            last_error = ServiceError(
                "service_unavailable",
                f"POST /research failed with HTTP {result['status']}",
                status=result["status"],
            )
            if attempt < 2 and time.monotonic() + 2.0 < deadline:
                time.sleep(2.0)
                continue
            raise last_error
        detail = (result["json"] or {}).get("detail") or result["text"][:200]
        raise ServiceError(
            "service_http",
            f"POST /research returned HTTP {result['status']}: {detail}",
            status=result["status"],
        )
    raise last_error or ServiceError("service_unavailable", "submit failed")


def poll(
    base_url: str,
    token: str,
    task_id: str,
    *,
    timeout_s: float,
    poll_s: float,
    poll_max_s: float,
    traceparent: str,
) -> tuple[dict, list[dict], float | None]:
    started = time.monotonic()
    deadline = started + timeout_s
    delay = max(0.25, poll_s)
    polls: list[dict] = []
    first_running_at: float | None = None
    terminal_body: dict | None = None
    while time.monotonic() < deadline:
        result = request(
            base_url,
            f"/result/{urllib.parse.quote(task_id)}",
            token=token,
            timeout_s=min(DEFAULT_HTTP_S, max(1.0, deadline - time.monotonic())),
            traceparent=traceparent,
        )
        status = result["status"]
        body = result["json"] or {}
        state = str(body.get("status") or "")
        polls.append(
            {
                "at_ms": int((time.monotonic() - started) * 1000),
                "http": status,
                "status": state,
                "ms": result["latency_ms"],
            }
        )
        if status in (401, 403):
            raise ServiceError("service_auth", f"poll rejected bearer token (HTTP {status})")
        if status == 404:
            raise ServiceError("service_unknown_task", f"unknown task id: {task_id}", status=404)
        if status == 429:
            time.sleep(retry_after_s(result["headers"], delay))
            continue
        if status != 200:
            if 500 <= status < 600 and time.monotonic() < deadline:
                time.sleep(min(delay, 5.0))
                continue
            raise ServiceError("service_http", f"poll returned HTTP {status}", status=status)
        if state in RUNNING and first_running_at is None:
            first_running_at = time.monotonic()
        if state in TERMINAL:
            terminal_body = body
            break
        if state not in QUEUED and state not in RUNNING:
            raise ServiceError("service_protocol", f"unknown job status: {state!r}")
        time.sleep(min(delay, max(0.25, deadline - time.monotonic())))
        delay = min(poll_max_s, delay * 1.6)
    if terminal_body is None:
        raise ServiceError("service_timeout", f"job {task_id} did not finish in {timeout_s}s")
    queue_wait_ms = None
    if first_running_at is not None:
        queue_wait_ms = int((first_running_at - started) * 1000)
    return terminal_body, polls, queue_wait_ms


def fetch_artifact(
    base_url: str, token: str, task_id: str, timeout_s: float, traceparent: str
) -> tuple[str, dict]:
    result = request(
        base_url,
        f"/result/{urllib.parse.quote(task_id)}/artifact",
        token=token,
        timeout_s=timeout_s,
        traceparent=traceparent,
        want_json=False,
    )
    if result["status"] == 409:
        raise ServiceError("service_not_done", "artifact requested before job completed", status=409)
    if result["status"] != 200:
        raise ServiceError("service_artifact", f"artifact HTTP {result['status']}", status=result["status"])
    return result["text"], result


def fetch_sources(
    base_url: str, token: str, task_id: str, timeout_s: float, traceparent: str
) -> tuple[list[dict], dict]:
    result = request(
        base_url,
        f"/result/{urllib.parse.quote(task_id)}/sources",
        token=token,
        timeout_s=timeout_s,
        traceparent=traceparent,
    )
    if result["status"] == 409:
        raise ServiceError("service_not_done", "sources requested before job completed", status=409)
    if result["status"] != 200:
        raise ServiceError("service_sources", f"sources HTTP {result['status']}", status=result["status"])
    body = result["json"] or {}
    sources = body.get("sources") or []
    if not isinstance(sources, list):
        raise ServiceError("service_protocol", "sources response is not a list")
    return sources, result


def source_count(sources: list) -> int:
    return len([s for s in sources if isinstance(s, dict)])


def count_citations(artifact: str) -> int:
    return len(set(re.findall(r"\[\^([^\]]+)\]", artifact)))


def frontmatter(artifact: str) -> dict:
    if not artifact.startswith("---"):
        return {}
    parts = artifact.split("---", 2)
    if len(parts) < 3:
        return {}
    block = parts[1]
    data: dict[str, str] = {}
    for line in block.splitlines():
        if ":" in line and not line.lstrip().startswith(("-", "#")):
            key, _, value = line.partition(":")
            key = key.strip().strip("'\"")
            data[key] = value.strip()
    return data


def evaluate(
    artifact: str,
    sources: list,
    expected: dict,
) -> tuple[list[str], dict]:
    failures: list[str] = []
    fm = frontmatter(artifact)
    required_keys = expected.get("frontmatter_keys") or [
        "@id",
        "type",
        "title",
        "provenance",
        "citations",
        "tldr_llm",
    ]
    missing = [key for key in required_keys if key not in fm]
    if not artifact.startswith("---"):
        failures.append("missing YAML frontmatter")
    elif missing:
        failures.append("frontmatter missing keys: " + ", ".join(missing))
    sections = len(re.findall(r"(?m)^##\s+\S", artifact))
    if sections < int(expected.get("min_sections", 2)):
        failures.append(f"too few sections: {sections}")
    citations = count_citations(artifact)
    if citations < int(expected.get("min_citations", 1)):
        failures.append(f"too few citations: {citations}")
    n_sources = source_count(sources)
    if n_sources < int(expected.get("min_sources", 1)):
        failures.append(f"too few sources: {n_sources}")
    if len(artifact) < int(expected.get("min_artifact_chars", 1)):
        failures.append(f"artifact too short: {len(artifact)}")
    for pattern in expected.get("forbidden_patterns", []):
        if re.search(str(pattern), artifact, re.IGNORECASE):
            failures.append(f"forbidden pattern: {pattern}")
    return failures, {
        "citations": citations,
        "nsources": n_sources,
        "sections": sections,
        "frontmatter_keys": sorted(fm),
    }


def scan_usage(store: str, project: str, tag: str, since_ms: int, until_ms: int) -> dict:
    if not store or not tag:
        return {"available": False, "reason": "usage store/tag not configured"}
    usage_dir = Path(store) / "usage" / project
    events: list[dict] = []
    for path in sorted(usage_dir.glob("events-*.jsonl")):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for line in lines:
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("event") != "llm.call":
                continue
            if event.get("tag") != tag:
                continue
            started = event.get("started_at_ms")
            if not isinstance(started, int) or not (since_ms - 2000 <= started <= until_ms + 2000):
                continue
            events.append(event)
    if not events:
        return {"available": False, "reason": "no proxy events matched this run tag"}
    input_tokens = sum(e["input_tokens"] for e in events if isinstance(e.get("input_tokens"), int))
    output_tokens = sum(
        e["output_tokens"] for e in events if isinstance(e.get("output_tokens"), int)
    )
    models = sorted({str(e.get("model")) for e in events if e.get("model")})
    outcomes: dict[str, int] = {}
    for event in events:
        outcome = str(event.get("outcome", "unknown"))
        outcomes[outcome] = outcomes.get(outcome, 0) + 1
    return {
        "available": True,
        "events": len(events),
        "input_tokens": input_tokens if input_tokens else None,
        "output_tokens": output_tokens if output_tokens else None,
        "models": models,
        "outcomes": outcomes,
    }


def enforce_free_models(
    usage: dict, allowlist: list[str], *, required: bool
) -> tuple[bool, list[str]]:
    if not required:
        return True, []
    if not usage.get("available"):
        return False, ["free-model policy cannot be verified: no usage events"]
    allowed = {m.strip() for m in allowlist if m.strip()}
    if not allowed:
        return False, ["free-model policy has an empty allowlist"]
    served = set(usage.get("models") or [])
    disallowed = sorted(served - allowed)
    if disallowed:
        return False, ["non-free model served: " + ", ".join(disallowed)]
    return True, []


def write_artifact(artifact: str, out_dir: str, trace_id: str) -> str:
    if not out_dir:
        return ""
    directory = Path(out_dir)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{trace_id}-{uuid.uuid4().hex[:6]}.md"
    path.write_text(artifact, encoding="utf-8")
    return str(path)


def run(args: argparse.Namespace) -> dict:
    token = service_token()
    topic = load_topic(args.dataset, args.topic)
    expected = dict(topic.get("expected") or {})
    base_url = args.base_url or os.environ.get("EVAL_RESEARCH_SERVICE_URL", "")
    if not base_url:
        return {"status": "error", "code": "missing_service_url", "failures": []}
    if not args.health_only and not token:
        return {"status": "error", "code": "missing_service_token", "failures": []}
    trace_id = args.trace_id or ""
    if args.traceparent and len(args.traceparent.split("-")) >= 2:
        trace_id = args.traceparent.split("-")[1]
    started_at = now_ms()
    result: dict = {
        "lane": args.lane,
        "topic": args.topic,
        "query": str(topic.get("query") or ""),
        "kind": "health" if args.health_only else "research",
        "status": "fail",
        "code": None,
        "failures": [],
        "service": {
            "host": urllib.parse.urlparse(base_url).netloc,
            "health": None,
            "readiness": None,
            "submit": None,
            "poll_count": 0,
            "queue_wait_ms": None,
            "e2e_ms": None,
            "error_class": None,
        },
        "task_id": None,
        "thread_id": args.thread_id or f"eval-service-{args.topic}-{uuid.uuid4().hex[:8]}",
        "artifact_chars": 0,
        "artifact_sha256": hashlib.sha256(b"").hexdigest(),
        "artifact_path": "",
        "citations": 0,
        "nsources": 0,
        "usage": {"available": False, "reason": "not collected"},
        "internal_spans": "n/a",
        "trace_id": trace_id,
        "trace_exported": False,
        "traceparent_propagated": False,
        "model_policy": {
            "required": args.require_free_models,
            "allowlist": [m for m in args.free_model_allowlist.split(",") if m],
            "verified": None,
        },
    }
    try:
        result["service"]["health"] = health(base_url, args.http_timeout_s, args.traceparent)
        result["service"]["readiness"] = readiness(base_url, args.http_timeout_s, args.traceparent)
        ready = effective_ready(args.lane, result["service"]["health"], result["service"]["readiness"])
        result["service"]["effective_ready"] = ready
        if args.health_only:
            result["status"] = "ready" if ready else "not_ready"
            result["code"] = None if ready else "service_not_ready"
            return result
        if not ready:
            raise ServiceError("service_not_ready", "health/readiness checks failed")
        submit_query = str(topic["query"])
        if args.lane == "remote":
            submit_query = f"{submit_query} [eval:{result['thread_id']}]"
        submit_info = submit(
            base_url,
            token,
            submit_query,
            result["thread_id"],
            args.timeout_s,
            args.traceparent,
        )
        result["task_id"] = submit_info["task_id"]
        result["service"]["submit"] = {
            "status": submit_info["status"],
            "ms": submit_info["ms"],
            "attempts": submit_info["attempts"],
        }
        terminal, polls, queue_wait = poll(
            base_url,
            token,
            result["task_id"],
            timeout_s=args.timeout_s,
            poll_s=args.poll_s,
            poll_max_s=args.poll_max_s,
            traceparent=args.traceparent,
        )
        result["service"]["poll_count"] = len(polls)
        result["service"]["queue_wait_ms"] = queue_wait
        result["service"]["e2e_ms"] = now_ms() - started_at
        if terminal.get("status") != "done":
            message = str(terminal.get("error") or "job failed")
            raise ServiceError("service_job_error", message[:800])
        if terminal.get("thread_id"):
            result["thread_id"] = str(terminal["thread_id"])
        artifact, artifact_response = fetch_artifact(
            base_url, token, result["task_id"], args.http_timeout_s, args.traceparent
        )
        sources, sources_response = fetch_sources(
            base_url, token, result["task_id"], args.http_timeout_s, args.traceparent
        )
        result["artifact_chars"] = len(artifact)
        result["artifact_sha256"] = hashlib.sha256(artifact.encode("utf-8")).hexdigest()
        result["artifact_path"] = write_artifact(artifact, args.artifact_out, trace_id or "service")
        result["citations"] = count_citations(artifact)
        result["nsources"] = source_count(sources)
        result["sources"] = sources
        failures, details = evaluate(artifact, sources, expected)
        result["artifact"] = details
        result["http"] = {
            "artifact_status": artifact_response["status"],
            "sources_status": sources_response["status"],
            "artifact_bytes": artifact_response["bytes"],
        }
        result["service"]["e2e_ms"] = now_ms() - started_at
        usage = scan_usage(
            args.usage_store,
            args.usage_project,
            args.usage_tag,
            started_at,
            now_ms(),
        )
        result["usage"] = usage
        free_ok, free_failures = enforce_free_models(
            usage,
            [m for m in args.free_model_allowlist.split(",") if m],
            required=args.require_free_models,
        )
        result["model_policy"]["verified"] = free_ok
        failures.extend(free_failures)
        result["failures"] = failures
        result["status"] = "pass" if not failures else "fail"
        result["code"] = None if not failures else "expectations_failed"
        result["internal_spans"] = (
            "captured_by_target_otel"
            if args.lane == "local" and args.trace_endpoint
            else "n/a"
        )
        result["trace_exported"] = bool(args.trace_endpoint)
        return result
    except ServiceError as exc:
        result["status"] = "fail"
        result["code"] = exc.code
        result["failures"] = [redact(str(exc), token)]
        result["service"]["error_class"] = exc.code
        result["service"]["e2e_ms"] = now_ms() - started_at
        result["usage"] = scan_usage(
            args.usage_store,
            args.usage_project,
            args.usage_tag,
            started_at,
            now_ms(),
        )
        return result


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", default="")
    parser.add_argument("--lane", choices=("local", "remote"), default="remote")
    parser.add_argument("--dataset", default=DEFAULT_DATASET)
    parser.add_argument("--topic", default=DEFAULT_TOPIC)
    parser.add_argument("--thread-id", default="")
    parser.add_argument("--timeout-s", type=float, default=DEFAULT_TIMEOUT_S)
    parser.add_argument("--poll-s", type=float, default=DEFAULT_POLL_S)
    parser.add_argument("--poll-max-s", type=float, default=DEFAULT_POLL_MAX_S)
    parser.add_argument("--http-timeout-s", type=float, default=DEFAULT_HTTP_S)
    parser.add_argument("--artifact-out", default="")
    parser.add_argument("--traceparent", default="")
    parser.add_argument("--trace-id", default="")
    parser.add_argument("--trace-endpoint", default="")
    parser.add_argument("--health-only", action="store_true")
    parser.add_argument("--usage-store", default="")
    parser.add_argument("--usage-project", default="research-agents")
    parser.add_argument("--usage-tag", default="")
    parser.add_argument("--free-model-allowlist", default="")
    parser.add_argument("--require-free-models", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    result = run(parse_args(argv))
    sys.stdout.write(json.dumps(result, ensure_ascii=False) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())


