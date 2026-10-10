#!/usr/bin/env python3
"""eval/v1 target adapter for the LLM-call examples.

Case contract (``input.params``):

- ``task``: ``extract`` | ``classify`` | ``summarize``.
- ``text`` (or ``input.stdin_text``): the model input.
- ``provider``: ``fake`` (default) | ``live``.

Output envelope: ``output.text`` carries the primary answer,
``output.data`` the structured payload. Live runs attach an ``ai``
block (model + token usage when reported); offline runs omit it —
absent means n/a, never zero. Credentials never leave the process.
"""

import json
import sys

from provider import ProviderError, select_provider


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
    text = params.get("text", (case.get("input") or {}).get("stdin_text", ""))
    provider_name = params.get("provider", "fake")
    try:
        provider = select_provider(provider_name)
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
