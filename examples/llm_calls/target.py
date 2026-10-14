#!/usr/bin/env python3
"""eval/v1 target adapter for the LLM-call examples.

Case contract (``input.params``):

- ``task``: ``extract`` | ``classify`` | ``summarize``.
- ``text`` (or ``input.stdin_text``): the model input.
- ``provider``: ``fake`` (default) | ``live``.
- ``variant``: ``a`` (concise) | ``b`` (verbose, default) — fixture
  model personality for benchmarks.

Output envelope: ``output.text`` carries the primary answer,
``output.data`` the structured payload. Live runs attach an ``ai``
block (model + token usage when reported); offline runs omit it —
absent means n/a, never zero. Credentials never leave the process.
"""

import json
import os
import sys

from provider import ProviderError, select_provider

# Live providers persist for the process lifetime so request budgets
# span cases in worker mode (one app process, one budget). Fake
# providers are stateless; caching them is harmless.
_PROVIDER_CACHE = {}


def cached_provider(name, variant="b", role="candidate"):
    import os as _os

    if name != "live":
        return select_provider(name, variant=variant)
    # Judge calls get their own budget pool so candidate and judge
    # consumption never share one counter silently.
    key = (
        role,
        _os.environ.get("EVAL_LLM_ENDPOINT", ""),
        _os.environ.get("EVAL_LLM_MODEL", ""),
        _os.environ.get("EVAL_LLM_MAX_REQUESTS", ""),
    )
    if key not in _PROVIDER_CACHE:
        _PROVIDER_CACHE[key] = select_provider(name)
    return _PROVIDER_CACHE[key]


def emit(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def die(message):
    sys.stderr.write(f"llm_target: {message}\n")
    sys.stderr.flush()
    sys.exit(2)


def primary_text(task, result):
    if task == "classify":
        return result if isinstance(result, str) else json.dumps(result)
    if isinstance(result, dict) and "summary" in result:
        return result["summary"]
    return json.dumps(result)


def handle_case(case):
    params = (case.get("input") or {}).get("params") or {}
    task = params.get("task", "extract")
    if task == "digest_judge":
        handle_judge(case, params)
        return
    text = params.get("text", (case.get("input") or {}).get("stdin_text", ""))
    provider_name = params.get("provider", "fake")
    try:
        # Task association for usage events (only read when recording).
        os.environ["EVAL_USAGE_TASK"] = case["case_id"]
        provider = cached_provider(provider_name, params.get("variant", "b"))
        result = provider.complete(task, text)
    except ProviderError as error:
        emit(
            {
                "type": "case_error",
                "case_id": case["case_id"],
                "code": str(error),
                "message": f"provider {provider_name} failed: {error}",
                "retriable": False,
            }
        )
        return
    usage = result.pop("_usage", None) if isinstance(result, dict) else None
    if usage is None:
        # Classify answers are bare strings; usage travels beside them.
        usage = getattr(provider, "last_usage", None)
    message = {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {
            "text": primary_text(task, result),
            "data": result,
            "files": [],
        },
    }
    if usage:
        ai = {"model": usage.get("model")}
        if usage.get("tokens_in") is not None:
            ai["tokens_in"] = usage["tokens_in"]
        if usage.get("tokens_out") is not None:
            ai["tokens_out"] = usage["tokens_out"]
        message["ai"] = ai
    emit(message)


#: Judge rubric id (versioned). Shape follows digest_agent's own
#: per-PR review rubric (SCORE / VERDICT / CRITIQUE, PASS bar >= 8).
JUDGE_RUBRIC = "digest-mr-review/v1"

JUDGE_FOCUS = {
    "accuracy": "1. Grounded in diff. 2. refs correct. 3. inferred code labeled.",
    "detail": "1. names files/functions/PRs. 2. real code snippets. 3. diffscope depth.",
    "structure": "1. one title, sections. 2. all areas. 3. no hype.",
}


def judge_prompt(section, focus):
    criteria = JUDGE_FOCUS.get(focus, JUDGE_FOCUS["accuracy"])
    return (
        "You are a demanding reviewer evaluating a SINGLE pull-request section "
        "of a daily digest. Rubric (0-10): " + criteria + "\n\n"
        "SECTION:\n" + section + "\n\n"
        "Respond exactly:\nSCORE: <0-10>\nVERDICT: PASS|REVISE\n"
        "CRITIQUE:\n- <actionable>\n\nPASS only if well met (>=8)."
    )


def parse_judge_verdict(text):
    """Parse SCORE/VERDICT/CRITIQUE. Unknown shapes stay UNKNOWN."""
    import re

    score_match = re.search(r"SCORE:\s*(\d+(?:\.\d+)?)", text or "")
    verdict_match = re.search(r"VERDICT:\s*(PASS|REVISE)", (text or "").upper())
    score = None
    if score_match:
        try:
            score = float(score_match.group(1))
        except ValueError:
            score = None
    verdict = verdict_match.group(1) if verdict_match else "UNKNOWN"
    return score, verdict


def handle_judge(case, params):
    """Model-based judging with a versioned rubric (imperfect measurement).

    Fake provider: deterministic heuristic stand-in (documented).
    Live provider: a judge model grades the section; its costs are
    recorded as separate usage events under the judge model id.
    """
    section = params.get("section", "")
    focus = params.get("focus", "accuracy")
    provider_name = params.get("provider", "fake")
    judge_model = os.environ.get("EVAL_LLM_JUDGE_MODEL", "")
    try:
        os.environ["EVAL_USAGE_TASK"] = case["case_id"]
        if provider_name == "fake":
            verdict, score, justification = fake_judge(section)
            judge_id = "fake-judge/v1"
            distinct = True
        else:
            provider = cached_provider(provider_name, "b", "judge")
            if judge_model:
                provider.model = judge_model
            candidate_model = os.environ.get("EVAL_LLM_MODEL", provider.model)
            distinct = bool(judge_model) and judge_model != candidate_model
            raw = provider.complete_raw(judge_prompt(section, focus))
            text = raw if isinstance(raw, str) else json.dumps(raw)
            score, verdict = parse_judge_verdict(text)
            justification = text[:2000]
            judge_id = provider.model
        result = {
            "score": score,
            "verdict": verdict,
            "justification": justification,
            "judge_model": judge_id,
            "judge_distinct": distinct,
            "rubric": JUDGE_RUBRIC,
        }
    except ProviderError as error:
        emit({
            "type": "case_error", "case_id": case["case_id"],
            "code": str(error),
            "message": f"judge failed: {error}", "retriable": False,
        })
        return
    usage = getattr(provider, "last_usage", None) if provider_name != "fake" else None
    message = {
        "type": "result", "case_id": case["case_id"], "ok": True,
        "output": {"text": f"{verdict} ({score})", "data": result, "files": []},
    }
    if usage:
        message["ai"] = {
            "model": usage.get("model"),
            "tokens_in": usage.get("tokens_in"),
            "tokens_out": usage.get("tokens_out"),
        }
    emit(message)


def fake_judge(section):
    """Deterministic stand-in: PASS when the section looks like a real
    digest section (PR header + code fence), else REVISE."""
    has_header = "## PR #" in section
    has_code = "```" in section
    if has_header and has_code:
        return "PASS", 9.0, "heuristic: header and code present"
    return "REVISE", 3.0, "heuristic: missing header or code evidence"


def main():
    mode = "worker"
    handshook = False
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            die("expected JSON")
        if not handshook:
            if msg.get("protocol") != "eval/v1":
                die("version mismatch")
            mode = msg.get("mode", "worker")
            emit({"protocol": "eval/v1", "ready": True})
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        handle_case(msg)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
