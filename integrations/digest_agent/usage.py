#!/usr/bin/env python3
"""Usage-reporting decorator for digest_agent LLM providers.

Wraps any ``LLMProviderPort`` and emits one ``llm/v1`` event per
``complete()`` call to a JSONL file. What it measures honestly:

- per-call wall latency (timed around the call),
- outcome (ok / error / timeout) and error codes,
- model id as passed by the orchestrator,
- call counts and task association.

What it CANNOT report: token counts. The port returns a bare string;
provider usage blocks are discarded upstream, and this decorator will
not infer or fabricate tokens — those fields stay absent.

Events are labeled ``origin: synthetic`` when wrapped around fakes
and carry the given ``context`` (benchmark for eval runs).
"""

import contextlib
import json
import os
import time


class UsageReportingProvider:
    """Port-compatible wrapper emitting llm/v1 usage events."""

    def __init__(self, inner, usage_out, task_id, context="benchmark"):
        self._inner = inner
        self._usage_out = usage_out
        self._task_id = task_id
        self._context = context
        self._seq = 0

    def list_free_models(self):
        return self._inner.list_free_models()

    def test(self, model_id):
        return self._inner.test(model_id)

    def complete(self, model_id, prompt):
        request_id = f"{self._task_id}:req-{self._seq}"
        self._seq += 1
        started_ms = int(time.time() * 1000)
        started = time.perf_counter()
        try:
            text = self._inner.complete(model_id, prompt)
        except Exception as error:
            duration_ms = int((time.perf_counter() - started) * 1000)
            self._emit(
                request_id, model_id, started_ms, duration_ms,
                "error", type(error).__name__, False,
            )
            raise
        duration_ms = int((time.perf_counter() - started) * 1000)
        self._emit(
            request_id, model_id, started_ms, duration_ms,
            "ok", None, False,
        )
        return text

    def _emit(self, request_id, model_id, started_ms, duration_ms,
              outcome, error_code, retriable):
        # event_id carries a random suffix: two trials of the same case
        # share task_id and request numbering, but each physical
        # emission must be unique (redelivery of the SAME id stays a
        # duplicate by design).
        import uuid

        event = {
            "event": "llm.call",
            "event_version": 1,
            "event_id": f"{request_id}:attempt-0:{uuid.uuid4().hex[:8]}",
            "project": "digest-agent",
            "task_id": self._task_id,
            "request_id": request_id,
            "attempt": 0,
            "provider": "fake",
            "model": str(model_id),
            "started_at_ms": started_ms,
            "duration_ms": duration_ms,
            "streaming": False,
            "outcome": outcome,
            "retriable": retriable,
            "origin": "synthetic",
            "context": self._context,
        }
        if error_code is not None:
            event["error_code"] = error_code
        append_jsonl(self._usage_out, event)


def append_jsonl(path, event):
    """Append one event safely across concurrent processes.

    ``O_APPEND`` alone is NOT sufficient: CPython on Windows opens the
    file without atomic append positioning, so two processes opening an
    empty file together both write at offset 0 and one append is lost.
    A one-byte region lock serializes open-seek-write-close instead.
    Portable: ``msvcrt`` on Windows, ``fcntl.flock`` elsewhere.
    """
    data = (json.dumps(event) + "\n").encode("utf-8")
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
