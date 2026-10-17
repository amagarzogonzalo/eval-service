# Integrating a project

How to make any project evaluable. Read `eval schema` first — it is the
machine-readable version of everything below and is generated from the
engine's own definitions, so it cannot drift. This document explains the
*why*; the manifest carries the *what*.

```
eval schema            # graders, field paths, protocol messages, exit codes
```

## The model in one paragraph

The engine **spawns your program as a subprocess** and speaks JSON Lines to
it over stdin/stdout. Your adapter translates between that protocol and your
tool. The engine owns execution, timeouts, grading, storage, comparison and
reporting; it never learns anything about your project. That is why the same
engine serves a Python CLI, a LangGraph agent, a Rust service or an API
contract test — and why adding a project never touches `crates/`.

```
suite.yaml ──► eval run ──► spawns adapter ──► your tool
   (cases,        │            (eval/v1 JSONL)
    graders)      ▼
              grades + usage ──► eval-runs/ ──► eval compare / dashboard
```

## Three patterns — pick one

| Pattern | When | Template |
|---|---|---|
| **CLI** | a command exists and has an exit code + stdout | `eval init NAME --template cli` |
| **LLM** | you call a model and want tokens/latency/cost recorded | `eval init NAME --template llm` |
| **Agent** | a multi-step workflow; grade its steps and budget | `eval init NAME --template agent` |

Every template is **runnable the moment it is scaffolded**: `eval check
--probe` and `eval run` pass with zero edits. You replace exactly one
function and grow the suite.

## The loop (this is the whole workflow)

```sh
cd path/to/workdir                 # target/file paths resolve from the cwd you run eval in
eval init mytool --template cli    # scaffold: adapter/target.py + suite.yaml + README.md
cd mytool

eval check suite.yaml --probe      # 1. deterministic gate: suite valid + adapter handshake OK
#   ... edit adapter/target.py: replace the one marked function ...
eval check suite.yaml --probe      # repeat until exit 0

eval run suite.yaml --ref baseline --set-baseline   # 2. freeze a baseline
#   ... change your tool ...
eval run suite.yaml --ref work                       # 3. candidate
eval compare --suite suite.yaml                      # 4. gate: 0 pass / 1 regression / 2 infra
```

`eval check --probe` is the highest-signal command. It validates the suite
(bad regex, inverted range, stability without trials, unknown field roots)
and live-tests the adapter's `eval/v1` handshake, returning stable issue
codes and a fix hint for each. Converge on it before you expect grading to
mean anything.

## The adapter contract (exact)

The engine writes JSONL to stdin, reads JSONL from stdout. `stderr` is
free-form logs and is never parsed. The exchange:

```jsonc
// engine → you, once
{"protocol":"eval/v1","mode":"worker"}
// you → engine, immediately
{"protocol":"eval/v1","ready":true}
// engine → you, once per case
{"type":"case","case_id":"validate-ok",
 "input":{"argv":["validate","resume.yaml"],"stdin_text":"","params":{}},
 "limits":{"timeout_ms":10000,"max_output_bytes":65536},
 "env":{"OFFLINE":"1"}}
// you → engine: zero or more events (seq 0,1,2,… gapless), then ONE terminal
{"type":"event","case_id":"validate-ok","seq":0,"name":"stage_done","data":{}}
{"type":"result","case_id":"validate-ok","ok":true,
 "output":{"text":"score=71.2","files":["out/report.json"],"data":{}},
 "metrics":{"wall_ms":4120},
 "ai":{"model":"gpt-4o-mini","tokens_in":1200,"tokens_out":300}}
// OR the handled-failure terminal (exit stays 0):
{"type":"case_error","case_id":"validate-ok","code":"no_sources",
 "message":"fixture directory empty","retriable":false}
```

The engine distinguishes, without any help from you:

| Situation | Your behavior | Engine records |
|---|---|---|
| success | one `result` | graded `ok` |
| handled failure | one `case_error` | graded failure with `code` |
| deadline exceeded | (nothing) | `Timeout`, worker killed + respawned |
| nonzero exit before terminal | — | `Crash` with the real exit code |
| non-JSON / bad shape on stdout | — | `ProtocolError`, killed + respawned |

**Relay your tool's real exit code; don't swallow it.** A nonzero exit is a
`Crash` observation, and a suite grades the tool's genuine contract with an
`exit_code` grader. Handled domain negatives (bad input the tool correctly
rejects) should be `case_error`, not a crash.

## Graders — grade properties, never wording

The catalogue is in `eval schema` (with an example per kind). The rule for
model or free-text output: **never `exact`**. Use `json_schema` for structure,
`regex`/`range` for objective facts, `not_regex` for canaries/secrets that
must be absent, `judge_lite` for subjective quality, and `trials >= 2` +
`stability` when the target is nondeterministic.

Field paths reach anywhere in the observation: `status`, `exit_code`,
`wall_ms`, `stderr`, `output.text`, `output.<nested>`, `metrics.wall_ms`,
`ai.tokens_in`, `events[2].name`, `events.len`.

## Measuring LLM usage

Emit one `llm/v1` event per model call (schema in `docs/llm-events.md`), then:

```sh
eval usage-import events.jsonl --project mytool --tags... (see --help)
eval usage-report --project mytool
```

Token counts must come from the provider's own `usage` block. Absent metrics
stay `n/a` — never estimate, never zero-fill. Reported vs estimated cost are
kept distinct with the pricing version recorded.

## Joining the portfolio

Register the project so it appears in `eval projects` and the dashboard:

```toml
# eval-workspace.toml
[[project]]
id = "mytool"
name = "My Tool"
repo = "../mytool"            # relative to this file, or absolute
suites = ["eval/suite.yaml"]  # relative to the repo
store = "eval-runs"
```

Unavailable repos, missing suites and corrupt records are shown as
first-class rows — never silently dropped.

## Reference integrations

- `integrations/cv_toolkit/` — deterministic CLI (relay exit code, file
  artifacts, stability).
- `integrations/digest_agent/` — agent workflow: real orchestrator driven
  offline through its own fakes, a usage decorator for latency/outcomes, and
  a live mode gated behind explicit opt-in.
- `examples/llm_calls/` — LLM patterns: extraction, classification,
  summarization, judge calibration, and a two-variant benchmark.

## What an agent should do, concretely

1. `eval schema` — learn the graders, fields, protocol, exit codes.
2. `eval init <name> --template <pattern>` — get a green starting point.
3. Replace the one marked function in `adapter/target.py`.
4. `eval check <suite> --probe` — iterate to exit 0.
5. Add cases + graders that assert the properties you care about.
6. `eval run` baseline → change → `eval run` work → `eval compare`.
7. If it calls LLMs: emit `llm/v1` events and `eval usage-import`.
8. Register in `eval-workspace.toml`; `eval dashboard`.

No step requires editing `crates/`. If you believe one does, the integration
belongs as a config-driven grader or a custom adapter concern — file it, do
not fork the engine.
