"""Minimal W3C trace context and span export for the digest integration.

The target project has no tracing hooks and is never edited. The adapter
and driver wrap the real service calls (GitHub fetch, LLM calls, renderer,
notifier) and emit spans themselves.

Two sinks are supported, both optional and fail-soft:

- ``EVAL_TRACE_OUT``: append-only JSONL (``eval.trace/v1`` records). This is
  the local evidence file and is always written when the variable is set.
- ``EVAL_OTEL_ENDPOINT`` (also ``EVAL_TRACE_OTLP_ENDPOINT`` or
  ``OTEL_EXPORTER_OTLP_ENDPOINT``): an OTLP/HTTP JSON collector. Spans
  are batched and flushed at process exit.

Inbound context is a W3C ``traceparent`` from ``EVAL_TRACEPARENT`` /
``TRACEPARENT`` (or ``EVAL_TRACE_ID`` for the id alone). The adapter passes
a child ``traceparent`` to the driver, so one trace spans adapter, target
services, and (when the proxy understands the header) provider calls.

No prompts, completions, credentials, or message bodies are recorded.
Attributes are counts, durations, models, repo names, and outcomes only.
"""

import atexit
import contextlib
import json
import os
import secrets
import sys
import threading
import time
import urllib.request

TRACE_VERSION = "00"
DEFAULT_SERVICE = "digest-agent"
_SCHEMA = "eval.trace/v1"


def new_trace_id():
    return secrets.token_hex(16)


def new_span_id():
    return secrets.token_hex(8)


def parse_traceparent(value):
    """Return ``(trace_id, parent_span_id, sampled)`` or ``None``."""
    if not value:
        return None
    parts = value.strip().split("-")
    if len(parts) != 4:
        return None
    version, trace_id, span_id, flags = parts
    if version != TRACE_VERSION:
        return None
    if len(trace_id) != 32 or len(span_id) != 16:
        return None
    try:
        int(trace_id, 16)
        int(span_id, 16)
        sampled = bool(int(flags, 16) & 0x01)
    except ValueError:
        return None
    if trace_id == "0" * 32 or span_id == "0" * 16:
        return None
    return trace_id.lower(), span_id.lower(), sampled


def _attr_value(value):
    if isinstance(value, bool):
        return {"boolValue": value}
    if isinstance(value, int):
        return {"intValue": str(value)}
    if isinstance(value, float):
        return {"doubleValue": value}
    return {"stringValue": str(value)[:512]}


class Span:
    """A finished-or-open span. Child spans reference ``span_id``."""

    def __init__(self, tracer, name, trace_id, span_id, parent_span_id,
                 attributes, sampled):
        self.tracer = tracer
        self.name = name
        self.trace_id = trace_id
        self.span_id = span_id
        self.parent_span_id = parent_span_id
        self.attributes = dict(attributes)
        self.sampled = sampled
        self.start_ns = time.time_ns()
        self.end_ns = None
        self.status = "unset"
        self.error_type = None

    def set(self, **attributes):
        self.attributes.update(attributes)
        return self

    def traceparent(self):
        flags = "01" if self.sampled else "00"
        return f"00-{self.trace_id}-{self.span_id}-{flags}"

    def finish(self, status=None, error=None):
        if self.end_ns is not None:
            return
        self.end_ns = time.time_ns()
        if error is not None:
            self.status = "error"
            self.error_type = type(error).__name__
            self.attributes.setdefault("error.type", self.error_type)
            self.attributes.setdefault("error.message", str(error)[:300])
        elif status is not None:
            self.status = status
        elif self.status == "unset":
            self.status = "ok"
        self.tracer._emit(self)


class Tracer:
    """Process-local tracer; one trace id, many spans (thread-safe emits)."""

    def __init__(self, trace_id=None, parent_span_id=None, sampled=True,
                 service=DEFAULT_SERVICE, out_path="", otlp_endpoint="",
                 run_id="", enabled=True):
        self.trace_id = trace_id or new_trace_id()
        self.parent_span_id = parent_span_id
        self.sampled = sampled
        self.service = service or DEFAULT_SERVICE
        self.out_path = out_path or ""
        self.otlp_endpoint = _normalize_otlp(otlp_endpoint)
        self.run_id = run_id or ""
        self.enabled = enabled
        self._lock = threading.Lock()
        self._batch = []
        self._warned = False
        atexit.register(self.flush)

    @classmethod
    def from_env(cls):
        enabled = os.environ.get("EVAL_TRACE_DISABLE", "") not in ("1", "true", "yes")
        parsed = None
        for key in ("EVAL_TRACEPARENT", "TRACEPARENT", "HTTP_TRACEPARENT"):
            parsed = parse_traceparent(os.environ.get(key, ""))
            if parsed:
                break
        trace_id = parsed[0] if parsed else os.environ.get("EVAL_TRACE_ID") or None
        parent = parsed[1] if parsed else None
        sampled = parsed[2] if parsed else True
        return cls(
            trace_id=trace_id,
            parent_span_id=parent,
            sampled=sampled,
            service=os.environ.get("EVAL_TRACE_SERVICE", DEFAULT_SERVICE),
            out_path=os.environ.get("EVAL_TRACE_OUT", ""),
            otlp_endpoint=(
                os.environ.get("EVAL_TRACE_OTLP_ENDPOINT")
                or os.environ.get("EVAL_OTEL_ENDPOINT")
                or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT", "")
            ),
            run_id=os.environ.get("EVAL_TRACE_RUN_ID", ""),
            enabled=enabled,
        )

    def span(self, name, parent=None, **attributes):
        parent_id = parent.span_id if isinstance(parent, Span) else self.parent_span_id
        return Span(
            self, name, self.trace_id, new_span_id(), parent_id,
            attributes, self.sampled,
        )

    @contextlib.contextmanager
    def scope(self, name, parent=None, **attributes):
        span = self.span(name, parent=parent, **attributes)
        try:
            yield span
        except Exception as error:
            span.finish(error=error)
            raise
        else:
            span.finish()

    def flush(self):
        with self._lock:
            batch = self._batch
            self._batch = []
        if not batch or not self.otlp_endpoint:
            return
        payload = {
            "resourceSpans": [{
                "resource": {"attributes": self._resource_attrs()},
                "scopeSpans": [{
                    "scope": {"name": "eval-digest", "version": "1"},
                    "spans": batch,
                }],
            }],
        }
        request = urllib.request.Request(
            self.otlp_endpoint,
            data=json.dumps(payload).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=3) as response:
                response.read()
        except Exception as error:  # noqa: BLE001 - observability must never fail a run
            self._warn(f"otlp export failed: {type(error).__name__}")

    def _resource_attrs(self):
        attrs = [
            {"key": "service.name", "value": {"stringValue": self.service}},
            {"key": "service.version", "value": {"stringValue": "eval-service/digest-e2e"}},
            {"key": "eval.project", "value": {"stringValue": "digest-agent"}},
            {"key": "deployment.environment", "value": {"stringValue": "e2e"}},
        ]
        if self.run_id:
            attrs.append({"key": "eval.run_id", "value": {"stringValue": self.run_id}})
        return attrs

    def _emit(self, span):
        if not self.enabled:
            return
        record = {
            "schema": _SCHEMA,
            "service": self.service,
            "name": span.name,
            "trace_id": span.trace_id,
            "span_id": span.span_id,
            "parent_span_id": span.parent_span_id,
            "start_time_unix_nano": span.start_ns,
            "end_time_unix_nano": span.end_ns,
            "duration_ms": max(0, (span.end_ns - span.start_ns) // 1_000_000),
            "status": span.status,
            "attributes": span.attributes,
        }
        if self.run_id:
            record["run_id"] = self.run_id
        self._write_jsonl(record)
        if self.otlp_endpoint:
            otlp_span = {
                "traceId": span.trace_id,
                "spanId": span.span_id,
                "name": span.name,
                "kind": 1,
                "startTimeUnixNano": str(span.start_ns),
                "endTimeUnixNano": str(span.end_ns),
                "attributes": [
                    {"key": key, "value": _attr_value(value)}
                    for key, value in span.attributes.items()
                ],
                "status": {"code": 2 if span.status == "error" else 1},
            }
            if span.parent_span_id:
                otlp_span["parentSpanId"] = span.parent_span_id
            with self._lock:
                self._batch.append(otlp_span)

    def _write_jsonl(self, record):
        if not self.out_path:
            return
        line = (json.dumps(record, separators=(",", ":")) + "\n").encode("utf-8")
        try:
            directory = os.path.dirname(self.out_path)
            if directory:
                os.makedirs(directory, exist_ok=True)
            fd = os.open(self.out_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
            try:
                os.write(fd, line)
            finally:
                os.close(fd)
        except OSError as error:
            self._warn(f"trace jsonl write failed: {type(error).__name__}")

    def _warn(self, message):
        if self._warned:
            return
        self._warned = True
        sys.stderr.write(f"trace: {message}\n")
        sys.stderr.flush()


def _normalize_otlp(endpoint):
    endpoint = (endpoint or "").strip()
    if not endpoint:
        return ""
    if endpoint.endswith("/v1/traces"):
        return endpoint
    return endpoint.rstrip("/") + "/v1/traces"
