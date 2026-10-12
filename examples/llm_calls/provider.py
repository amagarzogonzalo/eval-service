#!/usr/bin/env python3
"""Provider interface for the LLM-call examples.

Mirrors the OpenAI-compatible ``chat/completions`` pattern used by the
sibling repositories (Bearer auth, ``{endpoint}/chat/completions``),
implemented with the standard library so the examples stay
dependency-free.

Two providers:

- ``FakeProvider``: deterministic fixtures. Default. No network, no keys.
- ``LiveProvider``: real HTTP calls. Requires ``EVAL_LLM_LIVE=1`` plus
  ``EVAL_LLM_API_KEY``. Never engaged implicitly.

Test seam: ``EVAL_LLM_MOCK_FILE`` points at a canned provider-response
JSON file used *instead of* the network. The opt-in and key guards
still apply — the mock only replaces the transport, so tests prove the
full live path short of the socket.
"""

import json
import os
import time
import urllib.request
import uuid
import contextlib

CHAT_COMPLETIONS_PATH = "/chat/completions"


class ProviderError(Exception):
    """Handled provider failure (maps to ``case_error``, exit stays 0)."""


class FakeProvider:
    """Deterministic fixture provider for offline evaluation.

    ``variant`` selects the fixture model personality ("a": concise,
    "b": verbose with citations). Token counts are a deterministic
    ``len // 4`` heuristic over real fixture text — fixture data, not
    measurements. Events are labeled synthetic; see the benchmark docs.
    """

    name = "fake"

    MODELS = {"a": "summarizer-a", "b": "summarizer-b"}

    def __init__(self, variant="b"):
        if variant not in self.MODELS:
            raise ProviderError(f"unknown fake variant: {variant}")
        self.variant = variant
        self.model = self.MODELS[variant]

    def complete(self, task, text):
        started_ms = int(time.time() * 1000)
        started = time.perf_counter()
        try:
            result = self._answer(task, text)
        except ProviderError:
            raise
        duration_ms = int((time.perf_counter() - started) * 1000)
        record_fake_usage(self.model, task, text, result, started_ms, duration_ms)
        return result

    def _answer(self, task, text):
        if task == "extract":
            return {
                "name": "Ada Lovelace",
                "email": "ada@example.com",
            }
        if task == "classify":
            lowered = text.lower()
            if any(word in lowered for word in ("match", "goal", "tournament", "league")):
                return "sports"
            if any(word in lowered for word in ("software", "chip", "server", "model")):
                return "tech"
            return "politics"
        if task == "summarize":
            if self.variant == "a":
                return {
                    "summary": "Council passed the transit budget for three lines.",
                    "keywords_covered": ["transit", "budget"],
                }
            return {
                "summary": (
                    "The council approved the transit budget after a long debate [1]. "
                    "Funding covers three new lines and night service, with "
                    "construction starting in spring [2]. Critics raised cost "
                    "concerns, but the measure passed with a clear majority, "
                    "and implementation oversight was assigned to the audit "
                    "committee for transparency."
                ),
                "keywords_covered": ["transit", "budget", "council"],
            }
        raise ProviderError(f"unknown task: {task}")


def record_fake_usage(model, task, text, result, started_ms, duration_ms):
    """Append one synthetic usage event when EVAL_USAGE_OUT is set.

    Token counts are ``len // 4`` over the real fixture input/output —
    deterministic and proportional, but heuristic fixture data.
    Labeled synthetic/benchmark so they can never pass as measured.
    """
    path = os.environ.get("EVAL_USAGE_OUT", "")
    if not path:
        return
    result_text = result if isinstance(result, str) else json.dumps(result)
    event = {
        "event": "llm.call",
        "event_version": 1,
        "event_id": f"fixture-{uuid.uuid4().hex[:8]}",
        "project": os.environ.get("EVAL_USAGE_PROJECT", "llm-examples"),
        "task_id": os.environ.get("EVAL_USAGE_TASK") or None,
        "request_id": f"fixture-{uuid.uuid4().hex[:8]}",
        "attempt": 0,
        "provider": "fixture",
        "model": model,
        "started_at_ms": started_ms,
        "duration_ms": duration_ms,
        "streaming": False,
        "input_tokens": max(1, len(text) // 4),
        "output_tokens": max(1, len(result_text) // 4),
        "outcome": "ok",
        "origin": "synthetic",
        "context": os.environ.get("EVAL_USAGE_CONTEXT", "benchmark"),
    }
    tag = os.environ.get("EVAL_USAGE_TAG", "")
    if tag:
        event["tag"] = tag
    append_jsonl(path, json.dumps(event))


class LiveProvider:
    """OpenAI-compatible HTTP provider. Explicit opt-in only."""

    name = "live"

    def __init__(self):
        if os.environ.get("EVAL_LLM_LIVE") != "1":
            raise ProviderError("live_not_enabled")
        self.api_key = os.environ.get("EVAL_LLM_API_KEY", "")
        if not self.api_key:
            raise ProviderError("missing EVAL_LLM_API_KEY")
        self.endpoint = os.environ.get("EVAL_LLM_ENDPOINT", "https://api.openai.com/v1")
        self.model = os.environ.get("EVAL_LLM_MODEL", "gpt-4o-mini")
        self.timeout = int(os.environ.get("EVAL_LLM_TIMEOUT_S", "30"))
        self.budget = int(os.environ.get("EVAL_LLM_MAX_REQUESTS", "5"))
        self.used = 0
        #: Usage of the most recent call (target.py merges it into the
        #: ai block; kept beside the result because classify answers
        #: are bare strings, not dicts).
        self.last_usage = None

    def complete(self, task, text):
        if self.used >= self.budget:
            raise ProviderError("request budget exhausted")
        self.used += 1
        body = {
            "model": self.model,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPTS[task]},
                {"role": "user", "content": text},
            ],
            "temperature": 0,
        }
        started_ms = int(time.time() * 1000)
        started = time.perf_counter()
        try:
            payload = self._post(body)
        except Exception as error:
            self._record(task, started_ms, elapsed_ms(started), "error",
                         type(error).__name__, None)
            raise ProviderError(f"provider request failed: {type(error).__name__}") from error
        duration_ms = elapsed_ms(started)
        choices = payload.get("choices") or []
        if not choices:
            self._record(task, started_ms, duration_ms, "error", "empty_choices", None)
            raise ProviderError("empty choices from provider")
        content = (choices[0].get("message") or {}).get("content", "")
        usage = payload.get("usage") or {}
        result = parse_task_output(task, content)
        self.last_usage = {
            "model": self.model,
            "tokens_in": usage.get("prompt_tokens"),
            "tokens_out": usage.get("completion_tokens"),
        }
        if isinstance(result, dict):
            result["_usage"] = dict(self.last_usage)
        self._record(task, started_ms, duration_ms, "ok", None, usage)
        return result

    def _post(self, body):
        """Single-attempt POST (no retries by design). Honors the mock
        transport when EVAL_LLM_MOCK_FILE is set (tests only)."""
        mock = os.environ.get("EVAL_LLM_MOCK_FILE", "")
        if mock:
            with open(mock, encoding="utf-8") as handle:
                return json.load(handle)
        request = urllib.request.Request(
            self.endpoint.rstrip("/") + CHAT_COMPLETIONS_PATH,
            data=json.dumps(body).encode("utf-8"),
            headers={
                "Authorization": "Bearer " + self.api_key,
                "Content-Type": "application/json",
            },
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            return json.loads(response.read().decode("utf-8"))

    def _record(self, task, started_ms, duration_ms, outcome, error_code, usage):
        """Append one llm/v1 event when EVAL_USAGE_OUT is set. Usage
        fields come from the provider response only — never estimated."""
        path = os.environ.get("EVAL_USAGE_OUT", "")
        if not path:
            return
        usage = usage or {}
        event = {
            "event": "llm.call",
            "event_version": 1,
            "event_id": f"{os.environ.get('EVAL_USAGE_TASK', 'live')}:req-{uuid.uuid4().hex[:8]}",
            "project": os.environ.get("EVAL_USAGE_PROJECT", "llm-examples"),
            "task_id": os.environ.get("EVAL_USAGE_TASK") or None,
            "request_id": f"live-{uuid.uuid4().hex[:8]}",
            "attempt": 0,
            "provider": "openai-compat",
            "model": self.model,
            "started_at_ms": started_ms,
            "duration_ms": duration_ms,
            "streaming": False,
            "outcome": outcome,
            "origin": "live",
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
    """Append one line safely across concurrent processes.

    ``O_APPEND`` alone is NOT sufficient: CPython on Windows opens the
    file without atomic append positioning, so two processes opening an
    empty file together both write at offset 0 and one append is lost
    (observed: ~30% loss with two concurrent writers). A one-byte
    region lock serializes open-seek-write-close instead. Portable:
    ``msvcrt`` on Windows, ``fcntl.flock`` elsewhere.
    """
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
            # Unlock at the originally locked offset.
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
    except ImportError:
        import fcntl

        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)


SYSTEM_PROMPTS = {
    "extract": (
        "Extract the person's name and email from the text. "
        "Reply with a single JSON object with keys 'name' and 'email' only."
    ),
    "classify": (
        "Classify the text into exactly one label: sports, tech, or politics. "
        "Reply with the label word only."
    ),
    "summarize": (
        "Summarize the text in 3-5 sentences covering transit, budget, and council. "
        "Reply with a single JSON object with keys 'summary' and 'keywords_covered' only."
    ),
}


def parse_task_output(task, content):
    """Normalize live text into the task's structured shape."""
    text = content.strip()
    if task == "classify":
        return text.split()[0].strip(".,\"'").lower() if text else ""
    start = text.find("{")
    end = text.rfind("}")
    if start >= 0 and end > start:
        try:
            parsed = json.loads(text[start : end + 1])
            if isinstance(parsed, dict):
                return parsed
        except json.JSONDecodeError:
            pass
    return {"raw": text}


def select_provider(name, variant="b"):
    if name == "live":
        return LiveProvider()
    if name == "fake":
        return FakeProvider(variant=variant)
    raise ProviderError(f"unknown provider: {name}")

