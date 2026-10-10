# Sample reports (real output, trimmed)

## `eval run` — passing suite (exit 0)

```text
suite cv-demo v1  ref baseline
run 1791633696858-f6ea5b05  4 cases / 5 trials  pass rate 1.00
  [ok  ] cv-validate-ok  [ok]  wall [81ms]
         pass exit_code[0] — exit_code[0] passed on 1/1 trials
         pass exact[output.text] — exact[output.text] passed on 1/1 trials
         pass file_exists[output/reports/cv_summary.json] — passed on 1/1 trials
         pass file_matches[output/reports/cv_summary.json] — passed on 1/1 trials
  [ok  ] cv-validate-rejects-bad-yaml  [crash]  wall [89ms]
         pass exit_code[2] — exit_code[2] passed on 1/1 trials
```

Note `cv-validate-rejects-bad-yaml`: the `[crash]` observation is a
deliberate nonzero exit graded by `exit_code` — the conventional-CLI
rejection pattern. It counts as a graded pass, not infra.

## `eval compare` — quiet rerun (exit 0)

```text
gate [PASS] cv-demo: baseline 1791633696858-f6ea5b05 vs candidate 1791633724587-7875777b
  what changed: no changes
  uncertainty: candidate Wilson 95% CI [0.510, 1.000] over 4 cases; paired bootstrap delta +0.000 95% CI [+0.000, +0.000] (seeded, 1000 reps); floor 0.05
```

## `eval compare` — regression (exit 1)

```text
gate [FAIL] cv-demo: baseline 1791633696858-f6ea5b05 vs candidate 1791633728840-213dced0
  what changed: 2 regressed case(s)
  affected: *suite*
  affected: cv-validate-ok
  evidence: `cv-validate-ok` grader file_matches[output/reports/cv_summary.json]: pass -> fail (trial 0: output/reports/cv_summary.json does not match "\"score\": 71")
  evidence: suite pass rate 0.750 below minimum 1.000
  uncertainty: candidate Wilson 95% CI [0.301, 0.954] over 4 cases; paired bootstrap delta -0.250 95% CI [-0.750, +0.000] (seeded, 1000 reps); floor 0.05
```

## `eval projects` — portfolio (exit 0)

```text
workspace eval-service examples (2 projects)
  [ok] CV demo (deterministic CLI) (cv-demo) — 2 suites, 6 cases (6 pass / 0 fail / 0 infra), regressed suites: 0
      suite cv-demo — latest cv-demo:work 1791634266156
      suite security-corpus — latest security-corpus:baseline 1791634266065
  [ok] Digest demo (bounded nondeterminism) (digest-demo) — 1 suites, 3 cases (3 pass / 0 fail / 0 infra), regressed suites: 0
      suite digest-demo — latest digest-demo:baseline 1791634265937
totals: 2/2 projects available, 9 cases pass / 0 fail / 0 infra
```

## Negative controls on one shared worker (exit 1)

```text
suite negative-controls v1  ref work
run 1791634589189-f343e830  6 cases / 6 trials  pass rate 0.33
  [ok  ] echo-before  [ok]  wall [1ms]
  [FAIL] boom-crash  [crash]  wall [2ms]
         FAIL exit_code[0] — trial 0: exit_code=3, expected 0
  [FAIL] hang-timeout  [timeout]  wall [820ms]
  [FAIL] bad-protocol  [protocol_error]  wall [2ms]
  [FAIL] schema-victim  [ok]  wall [0ms]
         FAIL json_schema[output.output] — trial 0: schema violations: $: missing required `missing_key`
  [ok  ] echo-after  [ok]  wall [0ms]
```

Every failure class lands in its own taxonomy slot with evidence, and
`echo-after` proves the shared worker was respawned cleanly each time.
