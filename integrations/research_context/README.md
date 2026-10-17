# research_context — eval integration (`cli`)

Wrap a command: replace `run_command` and keep relaying its real exit code.

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
