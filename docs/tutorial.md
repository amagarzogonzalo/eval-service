# Tutorial: zero to gated suite

Take a new project from nothing to a regression gate in ~15 minutes.
Worked example: a CV CLI (`datasets/cv_demo/suite.yaml`).

## 1. Scaffold

```sh
eval init mytool --dir .
# creates mytool/adapter/target.py, mytool/suite.yaml, mytool/README.md
```

The adapter is a ~25-line `eval/v1` target: read JSONL on stdin,
write JSONL on stdout, log to stderr. Replace `handle_case` with your
tool — wrap its outputs, never rewrite it.

## 2. Describe cases

```yaml
suite: mytool
version: 1
target:
  argv: ["$PYTHON", "adapter/target.py"]  # $PYTHON via EVAL_PYTHON
  mode: oneshot                            # or worker for persistent targets
env_allowlist: []
cases:
  - id: validate-ok
    input: { argv: ["validate", "resume.yaml"] }
    limits: { timeout_ms: 10000, max_output_bytes: 65536 }
    trials: 1
    graders:
      - kind: exit_code
        expect: 0
      - kind: file_matches
        path: output/reports/cv_summary.json
        pattern: '"score": 71'
```

Grader reference (all config-driven, no engine changes):

| kind | Asserts |
|---|---|
| `exit_code` | process exit code (also grades crashes: nonzero exit before a terminal message is a `Crash` observation) |
| `exact` / `regex` / `not_regex` | field equality / match / absence (canaries) |
| `range` | numeric field within `[min, max]` |
| `json_schema` | subset validator: `type`, `required`, `properties`, `enum`, `const` |
| `stability` | same normalized output across ≥2 trials (infra failures never count as stable) |
| `file_exists` / `file_matches` | artifact presence / content |
| `judge_lite` | deterministic text-quality score ≥ threshold (stand-in; live judges opt-in) |

Field paths: `status`, `exit_code`, `wall_ms`, `stderr`,
`output.<path>`, `metrics.<path>`, `ai.<path>`, `events[i].<path>`.

## 3. Baseline, change, gate

Run from the suite directory — target argv and file paths resolve
from your invocation cwd:

```sh
cd mytool
eval run suite.yaml --ref baseline --set-baseline   # exit 0
# ... change your tool ...
eval run suite.yaml --ref work                      # exit 0/1
eval compare --suite suite.yaml                     # exit 0/1/2
```

A failing gate names the changed cases and cites evidence lines;
re-running an unchanged suite stays quiet with identical digests.

## 4. Sampling and budgets (agent-shaped targets)

See `datasets/digest_demo/suite.yaml`: `trials: 5` with
`deterministic: false`, invariant graders (event budgets, verdict
presence) instead of exact matching, and `judge_lite` scores tracked
per trial. Gate statistics (Wilson CI + seeded paired bootstrap) stay
quiet on noise and fire on real degradation — proved by the synthetic
datasets in `crates/eval-graders/tests/`.

## 5. Security corpus

`datasets/security/suite.yaml` asserts canary tokens never leak into
outputs (`not_regex`) and secret shapes stay out of stored runs
(redaction is property-tested in `eval-core`).
