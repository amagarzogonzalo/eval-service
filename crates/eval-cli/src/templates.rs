//! `eval init` scaffolds: runnable integration templates.
//!
//! Each template is deliberately runnable the moment it is written — the
//! generated suite passes `eval check --probe` with zero edits, so an
//! agent (or human) starts from green and replaces one clearly marked
//! function. Pick the pattern that matches the project:
//!
//! - `cli`   wrap a subprocess command, relay its exit code
//! - `llm`   wrap a single model call, emit `llm/v1` usage
//! - `agent` drive a multi-step workflow, stream events
//!
//! The starter uses only the Python standard library, matching the
//! repository's zero-dependency target convention.

/// A generated scaffold: adapter script, suite, readme.
pub struct Scaffold {
    pub adapter: &'static str,
    pub suite: String,
    pub readme: String,
}

/// Build a scaffold for `template`, or `None` if the name is unknown.
pub fn scaffold(template: &str, name: &str) -> Option<Scaffold> {
    match template {
        "cli" => Some(Scaffold {
            adapter: CLI_ADAPTER,
            suite: cli_suite(name),
            readme: readme(name, "cli"),
        }),
        "llm" => Some(Scaffold {
            adapter: LLM_ADAPTER,
            suite: llm_suite(name),
            readme: readme(name, "llm"),
        }),
        "agent" => Some(Scaffold {
            adapter: AGENT_ADAPTER,
            suite: agent_suite(name),
            readme: readme(name, "agent"),
        }),
        _ => None,
    }
}

pub const TEMPLATE_NAMES: &[&str] = &["cli", "llm", "agent"];

const CLI_ADAPTER: &str = r#"#!/usr/bin/env python3
"""eval/v1 target: wrap a subprocess command and relay its exit code.

Replace `DEFAULT_COMMAND` (or pass `input.params.command`) with your tool.
Edit ONLY `run_command`; the protocol handling below is complete.

Rule: stdout is eval/v1 JSONL. Your tool's stdout goes inside the result
message; its stderr is forwarded to our stderr (logs, never parsed).
"""

import json
import os
import subprocess
import sys

#: Default command when a case does not set `input.params.command`.
DEFAULT_COMMAND = [sys.executable, "-c", "print('hello from cli')"]


def expand(token):
    """Support `$PYTHON` (this interpreter) and `$VAR` from the environment."""
    if token == "$PYTHON":
        return sys.executable
    if token.startswith("$"):
        return os.environ.get(token[1:], token)
    return token


def run_command(case):
    """Run the target and return (exit_code, stdout, stderr).

    This is the one function you replace. Anything you can spawn works.
    """
    raw = (case.get("input") or {}).get("params", {}).get("command") or DEFAULT_COMMAND
    command = [expand(part) for part in raw]
    try:
        proc = subprocess.run(command, capture_output=True, text=True, check=False)
    except OSError as error:
        # A target that cannot even start is a handled failure, not a crash.
        return 127, "", f"cannot run {command!r}: {error}"
    return proc.returncode, proc.stdout, proc.stderr


def handle_case(case):
    code, out, err = run_command(case)
    if err:
        sys.stderr.write(err)
        sys.stderr.flush()
    if code != 0:
        # Relay the real exit code: the engine records Crash(code) and a
        # suite grades it with an `exit_code` grader. Do NOT swallow it.
        return {
            "type": "case_error",
            "case_id": case["case_id"],
            "code": f"exit_{code}",
            "message": (err or out)[-500:] or f"command exited {code}",
            "retriable": False,
        }
    return {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {"text": out.strip(), "files": [], "data": None},
    }


def main():
    mode = "worker"
    handshook = False
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if not handshook:
            if msg.get("protocol") != "eval/v1":
                sys.stderr.write(f"version mismatch: {msg.get('protocol')}\n")
                sys.exit(2)
            mode = msg.get("mode", "worker")
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        print(json.dumps(handle_case(msg)), flush=True)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
"#;

const LLM_ADAPTER: &str = r#"#!/usr/bin/env python3
"""eval/v1 target: wrap one model call and emit llm/v1 usage.

Replace `call_model` with your provider call. Edit ONLY that function and,
if your provider reports usage, the `usage` fields — the protocol handling
below is complete.

Rule: stdout is eval/v1 JSONL. Token counts come from the provider's own
`usage` block; never estimate or fabricate them (absent stays absent).
"""

import json
import sys
import time

#: Set EVAL_USAGE_OUT to also append llm/v1 events for `eval usage-import`.
#: (Kept out of the default path so the scaffold runs anywhere.)


def call_model(task, text):
    """Return (result_payload, usage_or_None). Replace this.

    `result_payload` is your structured output; `usage` is
    {"model":..,"tokens_in":..,"tokens_out":..} when the provider reports it.
    """
    lowered = (text or "").lower()
    label = "sports" if "goal" in lowered else "other"
    return {"label": label, "input_echo": text[:80]}, None


def handle_case(case):
    params = (case.get("input") or {}).get("params") or {}
    task = params.get("task", "classify")
    text = params.get("text") or (case.get("input") or {}).get("stdin_text", "")
    started_ms = int(time.time() * 1000)
    try:
        payload, usage = call_model(task, text)
    except Exception as error:  # handled failure, not a crash
        return {
            "type": "case_error",
            "case_id": case["case_id"],
            "code": "model_error",
            "message": f"{type(error).__name__}: {error}",
            "retriable": True,
        }
    message = {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {"text": json.dumps(payload), "files": [], "data": payload},
        "metrics": {"wall_ms": int(time.time() * 1000) - started_ms},
    }
    if usage:
        message["ai"] = usage
    return message


def main():
    mode = "worker"
    handshook = False
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if not handshook:
            if msg.get("protocol") != "eval/v1":
                sys.stderr.write(f"version mismatch: {msg.get('protocol')}\n")
                sys.exit(2)
            mode = msg.get("mode", "worker")
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        print(json.dumps(handle_case(msg)), flush=True)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
"#;

const AGENT_ADAPTER: &str = r#"#!/usr/bin/env python3
"""eval/v1 target: drive a multi-step workflow and stream events.

Replace `run_pipeline` with your orchestrator. Emit one `event` per step
(seq 0, 1, 2, ... gapless) then exactly one terminal message. The runner
assembles the events into the trace and can grade on them (e.g. event
count, step names) — that is how you assert a workflow stayed in budget.
"""

import json
import sys


def run_pipeline(case, emit_event):
    """Replace this. `emit_event(seq, name, data)` records a step."""
    text = (case.get("input") or {}).get("stdin_text", "")
    emit_event(0, "step_fetch", {"chars": len(text)})
    emit_event(1, "step_analyse", {"tokens": len(text.split())})
    emit_event(2, "step_render", {})
    return {
        "type": "result",
        "case_id": case["case_id"],
        "ok": True,
        "output": {"text": f"processed {len(text.split())} words", "files": [], "data": None},
    }


def handle_case(case):
    events = []

    def emit_event(seq, name, data):
        events.append({"type": "event", "case_id": case["case_id"],
                       "seq": seq, "name": name, "data": data})

    terminal = run_pipeline(case, emit_event)
    # Events first, then the single terminal message: the runner requires
    # gapless seq per case and rejects a terminal before the events settle.
    for event in events:
        print(json.dumps(event), flush=True)
    return terminal


def main():
    mode = "worker"
    handshook = False
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        if not handshook:
            if msg.get("protocol") != "eval/v1":
                sys.stderr.write(f"version mismatch: {msg.get('protocol')}\n")
                sys.exit(2)
            mode = msg.get("mode", "worker")
            print(json.dumps({"protocol": "eval/v1", "ready": True}), flush=True)
            handshook = True
            continue
        if msg.get("type") == "shutdown":
            return
        print(json.dumps(handle_case(msg)), flush=True)
        if mode == "oneshot":
            return


if __name__ == "__main__":
    main()
"#;

fn cli_suite(name: &str) -> String {
    format!(
        r#"# Validate with:  eval check suite.yaml --probe
suite: {name}
version: 1
target:
  argv: ["$PYTHON", "adapter/target.py"]
  mode: oneshot
env_allowlist: []
gate:
  min_pass_rate: 1.0
cases:
  - id: cli-smoke
    tags: [cli, deterministic]
    input: {{ params: {{ command: ["$PYTHON", "-c", "print('hello from cli')"] }} }}
    limits: {{ timeout_ms: 10000, max_output_bytes: 65536 }}
    trials: 1
    deterministic: true
    graders:
      - kind: exit_code
        expect: 0
      - kind: exact
        field: output.text
        value: "hello from cli"
  # A nonzero exit is a graded outcome, not a crash: the adapter relays
  # the code and this grader asserts the tool's real rejection contract.
  - id: cli-rejects-bad-input
    tags: [cli, deterministic]
    input: {{ params: {{ command: ["$PYTHON", "-c", "import sys; sys.exit(2)"] }} }}
    limits: {{ timeout_ms: 10000, max_output_bytes: 65536 }}
    trials: 1
    deterministic: true
    graders:
      - kind: exit_code
        expect: 0   # the case_error maps to a handled failure; assert its code below
      - kind: exact
        field: error_code
        value: "exit_2"
  - id: cli-stable
    tags: [cli, stability]
    input: {{ params: {{ command: ["$PYTHON", "-c", "print('same')"] }} }}
    limits: {{ timeout_ms: 10000, max_output_bytes: 65536 }}
    trials: 2
    deterministic: true
    graders:
      - kind: stability
"#
    )
}

fn llm_suite(name: &str) -> String {
    format!(
        r#"# Validate with:  eval check suite.yaml --probe
# Grade properties, never exact wording, for model output.
suite: {name}
version: 1
target:
  argv: ["$PYTHON", "adapter/target.py"]
  mode: oneshot
env_allowlist: []
gate:
  min_pass_rate: 1.0
cases:
  - id: llm-structure
    tags: [llm, deterministic]
    input: {{ params: {{ task: classify, text: "the goal was disallowed" }} }}
    limits: {{ timeout_ms: 30000, max_output_bytes: 65536 }}
    trials: 1
    deterministic: true
    graders:
      - kind: exit_code
        expect: 0
      - kind: json_schema
        field: output.data
        schema:
          type: object
          required: [label, input_echo]
          properties:
            label: {{ type: string }}
      - kind: regex
        field: output.data.label
        pattern: "^(sports|other)$"
  - id: llm-label-expectation
    tags: [llm, deterministic]
    input: {{ params: {{ task: classify, text: "a goal in extra time" }} }}
    limits: {{ timeout_ms: 30000, max_output_bytes: 65536 }}
    trials: 1
    deterministic: true
    graders:
      - kind: exact
        field: output.data.label
        value: "sports"
  # Sampling case: repeated trials, invariants instead of equality, and
  # the gate uses Wilson CI + bootstrap so noise stays quiet.
  - id: llm-sampled
    tags: [llm, sampling]
    input: {{ params: {{ task: classify, text: "no signal here" }} }}
    limits: {{ timeout_ms: 30000, max_output_bytes: 65536 }}
    trials: 5
    deterministic: false
    graders:
      - kind: regex
        field: output.data.label
        pattern: "^(sports|other)$"
      - kind: stability
"#
    )
}

fn agent_suite(name: &str) -> String {
    format!(
        r#"# Validate with:  eval check suite.yaml --probe
# Grade the workflow's shape and budget, not just its final text.
suite: {name}
version: 1
target:
  argv: ["$PYTHON", "adapter/target.py"]
  mode: oneshot
env_allowlist: []
gate:
  min_pass_rate: 1.0
cases:
  - id: agent-happy-path
    tags: [agent, deterministic]
    input: {{ stdin_text: "three words here" }}
    limits: {{ timeout_ms: 30000, max_output_bytes: 65536 }}
    trials: 1
    deterministic: true
    graders:
      - kind: exit_code
        expect: 0
      - kind: range
        field: events.len
        min: 3
        max: 20        # budget guard: the loop must not run away
      - kind: regex
        field: events[0].name
        pattern: "^step_fetch$"
      - kind: regex
        field: output.text
        pattern: "processed \\d+ words"
  - id: agent-bound-is-stable
    tags: [agent, stability]
    input: {{ stdin_text: "same input" }}
    limits: {{ timeout_ms: 30000, max_output_bytes: 65536 }}
    trials: 2
    deterministic: true
    graders:
      - kind: stability
"#
    )
}

fn readme(name: &str, template: &str) -> String {
    let pattern = match template {
        "llm" => {
            "Wrap one model call: replace `call_model` and populate `ai` usage from the provider's own `usage` block."
        }
        "agent" => {
            "Drive a workflow: replace `run_pipeline` and emit one event per step, then return the terminal result."
        }
        _ => "Wrap a command: replace `run_command` and keep relaying its real exit code.",
    };
    format!(
        r#"# {name} — eval integration (`{template}`)

{pattern}

## Run

```sh
# 1. Verify the suite and the adapter's eval/v1 handshake (deterministic):
eval check suite.yaml --probe

# 2. Establish a baseline, then gate changes against it:
eval run suite.yaml --ref baseline --set-baseline
eval run suite.yaml --ref work
eval compare --suite suite.yaml      # exit 0 pass / 1 regression / 2 infra
```

## Learn the contract

```sh
eval schema            # machine-readable: grader kinds, field paths,
                       # protocol messages, exit codes — read this first
```

## Where things live

| Path | Purpose |
|---|---|
| `adapter/target.py` | the ONLY project-specific code; one function to replace |
| `suite.yaml` | cases + graders; `eval check` validates it deterministically |
| `eval-runs/` | local run history (git-ignored) |

The grader catalogue and field paths are in `eval schema`; the suite
schema and protocol shapes are there too. Nothing in the engine needs to
change to add this project.
"#
    )
}
