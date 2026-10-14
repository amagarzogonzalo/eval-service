#!/usr/bin/env python3
"""Python reference target speaking eval/v1 (worker + oneshot).

Behavior is driven per case by ``input.params.behavior`` and mirrors
``adapters/rust_shim`` exactly: echo (default), scrub, error, exit,
hang, crash, malformed, cv_write, digest_ok, digest_weak, digest_sample.

Stdout is JSONL only. Logs go to stderr (never parsed).
"""

import json
import re
import sys
import time

PROTOCOL = "eval/v1"


def emit(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def die(message):
    sys.stderr.write(f"python_shim: {message}\n")
    sys.stderr.flush()
    sys.exit(2)


def params_of(case):
    params = (case.get("input") or {}).get("params") or {}
    return params if isinstance(params, dict) else {}


def emit_event(case_id, seq, name, data):
    emit({"type": "event", "case_id": case_id, "seq": seq, "name": name, "data": data})


def emit_result(case_id, text, files):
    emit(
        {
            "type": "result",
            "case_id": case_id,
            "ok": True,
            "output": {"text": text, "files": files},
            "metrics": {"wall_ms": 1},
        }
    )


def good_summary(params):
    n = params.get("reviews", 3)
    return (
        f"Digest with {n} reviews processed within budget. "
        "Findings: latency p99 down 12% [1], error budget intact [2]. " + "detail " * 40
    )


def weak_summary():
    return "pending"


def scrub_canary(text):
    return re.sub(r"CANARY-[A-Za-z0-9]+", "CANARY-<SCRUBBED>", text)


def demo_payload(case, params, behavior, sampled_weak):
    stdin_text = (case.get("input") or {}).get("stdin_text", "")
    if behavior == "scrub":
        return scrub_canary(stdin_text), []
    if behavior == "cv_write":
        import os

        path = params.get("write_path", "output.txt")
        content = params.get("write_content", "{}")
        parent = os.path.dirname(path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(content)
        return params.get("text", "score=71.2"), [path]
    if behavior == "digest_ok":
        return good_summary(params), []
    if behavior == "digest_weak":
        return weak_summary(), []
    if behavior == "digest_sample":
        return (weak_summary(), []) if sampled_weak else (good_summary(params), [])
    if "text" in params:
        return params["text"], []
    return stdin_text, []


def handle_case(case):
    params = params_of(case)
    behavior = params.get("behavior", "echo")
    case_id = case["case_id"]
    if behavior == "hang":
        while True:
            time.sleep(3600)
    if behavior == "crash":
        sys.exit(3)
    if behavior == "exit":
        sys.exit(int(params.get("code", 1)))
    if behavior == "malformed":
        sys.stdout.write("this is not json at all\n")
        sys.stdout.flush()
        return
    if behavior == "error":
        emit(
            {
                "type": "case_error",
                "case_id": case_id,
                "code": params.get("code", "failed"),
                "message": params.get("message", "handled failure"),
                "retriable": bool(params.get("retriable", False)),
            }
        )
        return
    sampled_weak = behavior == "digest_sample" and (time.time_ns() % 5 == 4)
    text, files = demo_payload(case, params, behavior, sampled_weak)
    emit_event(case_id, 0, "stage_done", {"stage": "extract"})
    extra = int(params.get("reviews", 0))
    for i in range(extra):
        emit_event(case_id, i + 1, f"review_{i}", {"review": i, "budget": 6})
    if behavior in ("digest_ok", "digest_weak", "digest_sample"):
        verdict = "weak" if (behavior == "digest_weak" or sampled_weak) else "ok"
        emit_event(case_id, extra + 1, "verdict", {"verdict": verdict})
    emit_result(case_id, text, files)


def check_handshake(msg):
    if not isinstance(msg, dict) or "protocol" not in msg:
        die("handshake is not an object with a protocol")
    allowed = {"protocol", "mode"}
    unknown = set(msg) - allowed
    if unknown:
        die(f"handshake schema violation: unknown fields {sorted(unknown)}")
    if msg["protocol"] != PROTOCOL:
        die(f"version mismatch: {msg['protocol']}")
    if msg.get("mode", "worker") not in ("worker", "oneshot"):
        die("handshake schema violation: bad mode")
    return msg.get("mode", "worker")


def check_case(msg):
    if not isinstance(msg, dict) or msg.get("type") != "case":
        die("case schema violation")
    allowed = {"type", "case_id", "input", "limits", "env"}
    unknown = set(msg) - allowed
    if unknown:
        die(f"case schema violation: unknown fields {sorted(unknown)}")
    for key in ("case_id", "input", "limits"):
        if key not in msg:
            die(f"case schema violation: missing {key}")
    return msg


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
            die("handshake is not JSON")
        if not handshook:
            mode = check_handshake(msg)
            emit({"protocol": PROTOCOL, "ready": True})
            handshook = True
            continue
        if isinstance(msg, dict) and msg.get("type") == "shutdown":
            return
        handle_case(check_case(msg))
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
