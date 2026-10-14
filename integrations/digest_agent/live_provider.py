#!/usr/bin/env python3
"""Live OpenAI-compatible provider implementing digest_agent's
``LLMProviderPort`` — without touching that repository.

Configuration comes exclusively from the environment:

- ``EVAL_DIGEST_LIVE`` must be ``1`` (explicit opt-in).
- ``EVAL_DIGEST_API_KEY``: bearer credential (never logged/stored).
- ``EVAL_DIGEST_ENDPOINT``: base URL (default ``https://api.openai.com/v1``).
- ``EVAL_DIGEST_MODEL``: model id (required for live runs).
- ``EVAL_DIGEST_TIMEOUT_S`` (default 30), ``EVAL_DIGEST_MAX_REQUESTS``
  (default 5, per process).

Test seam: ``EVAL_DIGEST_MOCK_FILE`` points at a canned provider
response used *instead of* the network. Opt-in and key guards still
apply — the mock replaces the transport only, so tests prove the
full live path short of the socket.

Usage events (tokens from the provider ``usage`` block, never
estimated) append to ``EVAL_USAGE_OUT`` when set. Origin defaults to
``live``; mocked tests set ``EVAL_USAGE_ORIGIN=synthetic``.
"""

import contextlib
import json
import os
import time
import urllib.request
import uuid


class ProviderError(Exception):
    """Handled failure (maps to case_error in the eval adapter)."""


class LivePortProvider:
    """digest_agent-compatible provider with usage reporting."""

    name = "live"

    def __init__(self):
        if os.environ.get("EVAL_DIGEST_LIVE") != "1":
            raise ProviderError("live_not_enabled")
        self.api_key = os.environ.get("EVAL_DIGEST_API_KEY", "")
        if not self.api_key:
            raise ProviderError("missing EVAL_DIGEST_API_KEY")
        self.endpoint = os.environ.get("EVAL_DIGEST_ENDPOINT", "https://api.openai.com/v1")
        self.model = os.environ.get("EVAL_DIGEST_MODEL", "")
        if not self.model:
            raise ProviderError("missing EVAL_DIGEST_MODEL")
        self.timeout = int(os.environ.get("EVAL_DIGEST_TIMEOUT_S", "30"))
        self.budget = int(os.environ.get("EVAL_DIGEST_MAX_REQUESTS", "5"))
        self.used = 0
        self.origin = os.environ.get("EVAL_USAGE_ORIGIN", "live")

    def list_free_models(self):
        return [self.model]

    def test(self, model_id):
        from digest_agent.ports import TestResult

        try:
            text = self.complete(model_id, "Reply with the word ok.")
            passed = "ok" in text.lower()
            return TestResult(model_id=model_id, passed=passed,
                              reason="ok" if passed else "unexpected test reply")
        except ProviderError as error:
            return TestResult(model_id=model_id, passed=False, reason=str(error))

    def complete(self, model_id, prompt):
        if self.used >= self.budget:
            raise ProviderError("request budget exhausted")
        self.used += 1
        body = {"model": self.model, "messages": [{"role": "user", "content": prompt}],
                "temperature": 0}
        started_ms = int(time.time() * 1000)
        started = time.perf_counter()
        try:
            payload = self._post(body)
        except Exception as error:
            self._record(model_id, started_ms, elapsed_ms(started), "error",
                         type(error).__name__, None)
            raise ProviderError(f"provider request failed: {type(error).__name__}") from error
        duration_ms = elapsed_ms(started)
        choices = payload.get("choices") or []
        if not choices:
            self._record(model_id, started_ms, duration_ms, "error", "empty_choices", None)
            raise ProviderError("empty choices from provider")
        content = (choices[0].get("message") or {}).get("content", "")
        usage = payload.get("usage") or {}
        self._record(model_id, started_ms, duration_ms, "ok", None, usage)
        return content

    def _post(self, body):
        """Single attempt, no retries. Mock transport when configured."""
        mock = os.environ.get("EVAL_DIGEST_MOCK_FILE", "")
        if mock:
            with open(mock, encoding="utf-8") as handle:
                return json.load(handle)
        request = urllib.request.Request(
            self.endpoint.rstrip("/") + "/chat/completions",
            data=json.dumps(body).encode("utf-8"),
            headers={"Authorization": "Bearer " + self.api_key,
                     "Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            return json.loads(response.read().decode("utf-8"))

    def _record(self, model_id, started_ms, duration_ms, outcome, error_code, usage):
        path = os.environ.get("EVAL_USAGE_OUT", "")
        if not path:
            return
        usage = usage or {}
        event = {
            "event": "llm.call",
            "event_version": 1,
            "event_id": f"digest-live-{uuid.uuid4().hex[:8]}",
            "project": os.environ.get("EVAL_USAGE_PROJECT", "digest-agent"),
            "task_id": os.environ.get("EVAL_USAGE_TASK") or None,
            "request_id": f"digest-live-{uuid.uuid4().hex[:8]}",
            "attempt": 0,
            "provider": "openai-compat",
            "model": self.model,
            "started_at_ms": started_ms,
            "duration_ms": duration_ms,
            "streaming": False,
            "outcome": outcome,
            "origin": self.origin,
            "context": os.environ.get("EVAL_USAGE_CONTEXT", "benchmark"),
        }
        if error_code is not None:
            event["error_code"] = error_code
        if isinstance(usage.get("prompt_tokens"), int):
            event["input_tokens"] = usage["prompt_tokens"]
        if isinstance(usage.get("completion_tokens"), int):
            event["output_tokens"] = usage["completion_tokens"]
        tag = os.environ.get("EVAL_USAGE_TAG", "")
        if tag:
            event["tag"] = tag
        append_jsonl(path, json.dumps(event))


def elapsed_ms(started):
    return int((time.perf_counter() - started) * 1000)


def append_jsonl(path, line):
    """Region-locked append shared with usage.py (concurrent writers)."""
    data = (line + "\n").encode("utf-8")
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        with _file_lock(fd):
            os.lseek(fd, 0, os.SEEK_END)
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                view = view[written:]
    finally:
        os.close(fd)


@contextlib.contextmanager
def _file_lock(fd):
    try:
        import msvcrt

        os.lseek(fd, 0, os.SEEK_SET)
        msvcrt.locking(fd, msvcrt.LK_LOCK, 1)
        try:
            yield
        finally:
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
    except ImportError:
        import fcntl

        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
