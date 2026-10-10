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
"""

import json
import os
import urllib.request

CHAT_COMPLETIONS_PATH = "/chat/completions"


class ProviderError(Exception):
    """Handled provider failure (maps to ``case_error``, exit stays 0)."""


class FakeProvider:
    """Deterministic fixture provider for offline evaluation."""

    name = "fake"

    def complete(self, task, text):
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
        request = urllib.request.Request(
            self.endpoint.rstrip("/") + CHAT_COMPLETIONS_PATH,
            data=json.dumps(body).encode("utf-8"),
            headers={
                "Authorization": "Bearer " + self.api_key,
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except Exception as error:
            raise ProviderError(f"provider request failed: {type(error).__name__}") from error
        choices = payload.get("choices") or []
        if not choices:
            raise ProviderError("empty choices from provider")
        content = (choices[0].get("message") or {}).get("content", "")
        usage = payload.get("usage") or {}
        result = parse_task_output(task, content)
        result["_usage"] = {
            "model": self.model,
            "tokens_in": usage.get("prompt_tokens"),
            "tokens_out": usage.get("completion_tokens"),
        }
        return result


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


def select_provider(name):
    if name == "live":
        return LiveProvider()
    if name == "fake":
        return FakeProvider()
    raise ProviderError(f"unknown provider: {name}")
