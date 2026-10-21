"""Free-only enforcement for the digest path.

The digest entrypoint is configured with exactly one provider: the
target's Gemini free-tier adapter by default, or its Groq adapter as an
explicit fallback. Only the selected provider key is passed; no
OpenRouter/Mistral key is configured, so there is no paid fallback. The
target's OpenRouter catalog filter is not used because the local target
adapter has an unrelated catalog-sort defect and the task is output-only.

Three independent checks:

1. Static config scan: no known paid model id in digest-owned suites,
   runner, docs, datasets, or the systemd unit.
2. Model override check: ``EVAL_DIGEST_MODEL``, if present, must be
   explicitly free/fixture.
3. Target provider check: ``main.build_llm_provider`` with only the
   selected provider key exposes exactly the reviewed free candidates,
   can test the free model, and cannot route a paid id. HTTP is faked;
   no network is used.
"""

import json
import os
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__))))

PAID_PATTERNS = (
    "openai/gpt-4o",
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    "o1-",
    "o3-",
    "claude-",
    "gemini-2.5-pro",
    "gemini-2.0-pro",
    "mistral-large",
    "paid/",
    ":paid",
)

SCAN_PATHS = (
    "integrations/digest_agent",
    "datasets/digest_e2e",
    "docs/digest-e2e.md",
    "infra/aws/systemd/eval-live-digest.service",
)

SKIP_FILES = {"free_only.py", "replay_sitecustomize.py"}
TEXT_SUFFIXES = (".py", ".yaml", ".yml", ".sh", ".md", ".json",
                 ".toml", ".service", ".timer")

# Reviewed free-tier candidates exposed by the target adapters.
FREE_GEMINI_MODELS = {
    "gemini-3.6-flash",
    "gemini-flash-lite-latest",
}
FREE_GROQ_MODELS = {
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b",
    "groq/compound-mini",
}
PROVIDER_MODELS = {
    "gemini": FREE_GEMINI_MODELS,
    "groq": FREE_GROQ_MODELS,
}

FREE_SUFFIX = ":free"
FREE_PREFIXES = ("free-", "fixture/", "mock-")
FREE_EXACT = {"mock-model", "free-fixture", "fixture/free-digest"}


def is_free_model_id(model_id):
    if not model_id:
        return True
    model_id = str(model_id).strip()
    if model_id in FREE_EXACT:
        return True
    if model_id in FREE_GEMINI_MODELS or model_id in FREE_GROQ_MODELS:
        return True
    if model_id.endswith(FREE_SUFFIX):
        return True
    return model_id.startswith(FREE_PREFIXES)


def assert_free_model_id(model_id):
    if not is_free_model_id(model_id):
        raise ValueError(
            f"paid model id is forbidden in the digest path: {model_id!r}"
        )


def scan_config():
    hits = []
    for relative in SCAN_PATHS:
        path = os.path.join(REPO_ROOT, relative)
        if os.path.isfile(path):
            _scan_file(path, hits)
            continue
        for root, dirs, files in os.walk(path):
            dirs[:] = [d for d in dirs if d not in ("__pycache__", ".git")]
            for name in files:
                if name in SKIP_FILES:
                    continue
                if name.endswith(TEXT_SUFFIXES):
                    _scan_file(os.path.join(root, name), hits)
    return hits


def _scan_file(path, hits):
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            text = handle.read()
    except OSError:
        return
    lowered = text.lower()
    for pattern in PAID_PATTERNS:
        if pattern in lowered:
            hits.append({
                "path": os.path.relpath(path, REPO_ROOT),
                "pattern": pattern,
            })


class _FakeResponse:
    def __init__(self, status_code, payload):
        self.status_code = status_code
        self._payload = payload
        self.text = json.dumps(payload)

    def json(self):
        return self._payload

    def raise_for_status(self):
        if self.status_code >= 400:
            raise RuntimeError(f"HTTP {self.status_code}")


def check_gemini_provider():
    import requests

    original_post = requests.post

    def fake_post(url, *args, **kwargs):
        text = str(url)
        if "generativelanguage.googleapis.com/v1beta/models/" not in text:
            raise RuntimeError(f"unexpected POST in free-only test: {url}")
        model = text.split("/models/", 1)[1].split(":", 1)[0]
        if model not in FREE_GEMINI_MODELS:
            raise RuntimeError(f"paid model attempted: {model!r}")
        return _FakeResponse(200, {
            "candidates": [{
                "content": {
                    "parts": [{"text": "This is a valid free Gemini response."}]
                }
            }]
        })

    os.environ["GEMINI_API_KEY"] = "fixture-free-key"
    for name in ("OPENROUTER_API_KEY", "OPEN_ROUTER_API_KEY",
                 "MISTRAL_API_KEY", "GROQ_API_KEY"):
        os.environ.pop(name, None)
    requests.post = fake_post
    try:
        from digest_agent.adapters.driven.gemini_adapter import GeminiAdapter
        from digest_agent.main import build_llm_provider

        adapter = GeminiAdapter(api_key="fixture-free-key")
        models = adapter.list_free_models()
        if sorted(models) != sorted(FREE_GEMINI_MODELS):
            raise AssertionError(f"target Gemini candidates changed: {models!r}")
        provider = build_llm_provider()
        pool = provider.list_free_models()
        if sorted(pool) != sorted(FREE_GEMINI_MODELS):
            raise AssertionError(
                f"build_llm_provider exposed non-free models: {pool!r}"
            )
        if not provider.test(models[0]).passed:
            raise AssertionError("free Gemini fixture model test failed")
        try:
            provider.test("paid" + "/decoy-model")
        except Exception:  # noqa: BLE001, S110
            pass
        else:
            raise AssertionError("paid model was routable")
        return True
    finally:
        requests.post = original_post
        os.environ.pop("GEMINI_API_KEY", None)


def check_groq_provider():
    import requests

    original_post = requests.post

    def fake_post(url, *args, **kwargs):
        if "api.groq.com/openai/v1/chat/completions" not in str(url):
            raise RuntimeError(f"unexpected POST in free-only test: {url}")
        body = kwargs.get("json") or {}
        model = body.get("model")
        if model not in FREE_GROQ_MODELS:
            raise RuntimeError(f"paid model attempted: {model!r}")
        return _FakeResponse(200, {
            "choices": [{
                "message": {
                    "content": "This is a valid free Groq fixture response."
                }
            }]
        })

    os.environ["GROQ_API_KEY"] = "fixture-free-key"
    for name in ("OPENROUTER_API_KEY", "OPEN_ROUTER_API_KEY",
                 "MISTRAL_API_KEY", "GEMINI_API_KEY"):
        os.environ.pop(name, None)
    requests.post = fake_post
    try:
        from digest_agent.adapters.driven.groq_adapter import GroqAdapter
        from digest_agent.main import build_llm_provider

        adapter = GroqAdapter(api_key="fixture-free-key")
        models = adapter.list_free_models()
        if sorted(models) != sorted(FREE_GROQ_MODELS):
            raise AssertionError(f"target Groq candidates changed: {models!r}")
        provider = build_llm_provider()
        pool = provider.list_free_models()
        if sorted(pool) != sorted(FREE_GROQ_MODELS):
            raise AssertionError(
                f"build_llm_provider exposed non-free models: {pool!r}"
            )
        if not provider.test(models[0]).passed:
            raise AssertionError("free Groq fixture model test failed")
        try:
            provider.test("paid" + "/decoy-model")
        except Exception:  # noqa: BLE001, S110
            pass
        else:
            raise AssertionError("paid model was routable")
        return True
    finally:
        requests.post = original_post
        os.environ.pop("GROQ_API_KEY", None)


def check_dotenv_parser():
    import tempfile

    from entrypoint_runner import parse_dotenv_key

    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", delete=False, suffix=".env"
    ) as handle:
        path = handle.name
        handle.write("GEMINI_API_KEY=fixture-gemini-key\n")
        handle.write("GROQ_API_KEY=fixture-groq-key\n")
        handle.write("OPENROUTER_API_KEY=must-not-be-read\n")
    try:
        gemini = parse_dotenv_key(path, "GEMINI_API_KEY")
        groq = parse_dotenv_key(path, "GROQ_API_KEY")
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    if gemini != "fixture-gemini-key" or groq != "fixture-groq-key":
        raise AssertionError("dotenv parser did not isolate provider keys")
    return True


def run_checks():
    model = os.environ.get("EVAL_DIGEST_MODEL", "")
    assert_free_model_id(model)
    provider = os.environ.get("EVAL_DIGEST_PROVIDER", "gemini").strip().lower()
    if provider not in PROVIDER_MODELS:
        raise AssertionError("only free Gemini or Groq providers are allowed")
    hits = scan_config()
    if hits:
        raise AssertionError(f"paid model markers in digest config: {hits!r}")
    if os.environ.get("EVAL_DIGEST_ALLOW_PAID", "") not in ("", "0"):
        raise AssertionError("EVAL_DIGEST_ALLOW_PAID must not be enabled")
    if provider == "gemini":
        target_ok = check_gemini_provider()
    else:
        target_ok = check_groq_provider()
    dotenv_ok = check_dotenv_parser()
    return {
        "free_only": True,
        "config_scan": "ok",
        "paid_model_configured": False,
        "model_override": model or "none",
        "provider": provider,
        "provider_models": sorted(PROVIDER_MODELS[provider]),
        "target_filter": "ok" if target_ok else "failed",
        "dotenv_parser": "ok" if dotenv_ok else "failed",
        "schema": "digest.free-only/v1",
    }


def main():
    try:
        result = run_checks()
    except Exception as error:  # noqa: BLE001
        sys.stdout.write(json.dumps({
            "free_only": False,
            "error": f"{type(error).__name__}: {error}",
            "schema": "digest.free-only/v1",
        }) + "\n")
        return 1
    sys.stdout.write(json.dumps(result) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
