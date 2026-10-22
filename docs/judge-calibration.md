# Judge calibration (Phase 7)

A judge is a measurement instrument. Until it agrees with human labels,
its verdicts are **advisory**. `eval calibrate` measures a judge
offline against a human-labelled set and writes a versioned report;
it never calls a model and never promotes a judge automatically.

**Current status:** there is no human-labelled dataset. The shipped
fixture is synthetic, so the judge stays advisory, no report is used for
gating, and no labels are fabricated. Calibration is deferred until real
labels exist.

The repository decision H4 sets the bar: **kappa lower confidence bound
>= 0.6**, plus A/B position-swap and verbosity checks.

## Input: one JSON object per line

```json
{"id":"item-001","rubric":"digest-mr-review/v1","judge_model":"openai/gpt-oss-20b@2026-10","human_label":"pass","judge_label":"pass","human_score":9.0,"judge_score":8.8,"pair":{"a":"candidate A text","b":"candidate B text","human_winner":"a","judge_ab":{"winner":"a"},"judge_ba":{"winner":"a"}},"notes":"..."}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Unique row id. |
| `rubric` | recommended | Versioned rubric (e.g. `digest-mr-review/v1`). Mixed values warn. |
| `judge_model` | recommended | Pinned judge id. Mixed values warn. |
| `human_label` | for kappa | `pass` / `revise` (aliases `ok`, `fail` normalize). |
| `judge_label` | for kappa | Judge's absolute verdict. |
| `human_score` | optional | Human score on the rubric (0-10). |
| `judge_score` | for thresholds | Judge score on the rubric (0-10). |
| `pair` | for position checks | A/B pair with human preference and judge verdicts in both orders. |
| `notes` | optional | Free text; not scored. |

Pair rules:

- `a` and `b` are the candidate texts.
- `human_winner` is `a`, `b`, or `tie`.
- `judge_ab.winner` and `judge_ba.winner` are always in the **original**
  A/B orientation, regardless of presentation order. Swapping the
  order must not swap the labels in the data.
- A pair with only one order contributes to the pair count but not to
  the both-orders consistency check.

The sample `datasets/judge_calibration/digest_v1.jsonl` is **synthetic**:
it exercises the report, not a real model.

## Metrics

- **Cohen's kappa** over the human/judge absolute labels, with a
  deterministic 95% bootstrap percentile interval (1000 replicates by
  default). Undefined chance agreement is reported as `null`, never as
  0 or 1.
- **Threshold sweep**: for each threshold in 0.0..10.0 (0.5 steps),
  derive `pass = judge_score >= threshold`, compute accuracy and kappa
  against the human label, and report the configured threshold plus the
  best observed point. The sweep is diagnostic; do not tune a threshold
  on the same set used to claim performance.
- **A/B position swap**: order consistency, first-position rate, and
  agreement with the human winner in each order. A judge that follows
  the first position has a high first-position rate regardless of
  correctness.
- **Verbosity**: among pairs with unequal lengths, how often the judge
  and the human prefer the longer candidate. A large positive
  judge-minus-human gap is a warning and blocks gating eligibility.

## Decision

`gating_eligible` is true only when all hold:

- at least `--min-items` rows (default 30),
- kappa is defined and its lower CI is at least `--kappa-min` (0.6),
- at least `--min-position-items` both-order pairs (default 10),
- position consistency is at least `--min-position-consistency` (0.8),
- the verbosity gap is at most `--max-verbosity-gap` (0.2).

Otherwise the report is `advisory` and lists every reason. This is
deliberately conservative: a judge can be useful for diagnosis long
before it is safe for gating.

## Report

`--out` writes JSON with:

- `schema_version`, `report_id` (content-addressed; ignores generation
  time), `generated_at_ms`,
- dataset path, blake3, item count,
- pinned judge model, prompt version, rubric,
- parameters, agreement/CI, confusion counts, pair metrics, threshold
  sweep, verbosity diagnostics, warnings,
- the advisory/gating decision and reasons.

`--markdown` writes a human-readable summary. The report id is stable
for identical data and parameters, so a CI artifact can be compared
across runs.

## Command

```sh
eval calibrate datasets/judge_calibration/digest_v1.jsonl \
  --judge-model "openai/gpt-oss-20b@2026-10" \
  --prompt-version "judge-prompt-v3" \
  --rubric digest-mr-review/v1 \
  --out judge-calibration.json \
  --markdown judge-calibration.md
```

Useful flags:

- `--threshold` (default 8.0)
- `--kappa-min` (default 0.6)
- `--ci` (default 0.95)
- `--bootstrap` (default 1000)
- `--min-items`, `--min-position-items`,
  `--min-position-consistency`, `--max-verbosity-gap`
- `--require-gating`: exit 1 when the report is advisory (for a hard CI
  gate)
- `--json`: print the report to stdout

Exit codes: `0` report written (advisory allowed), `1` only with
`--require-gating` when the bar is not cleared, `2` invalid input or
I/O error.

## Producing judge labels

The command is offline by design. Produce the judge labels and scores
with the judge you are evaluating (for example an opt-in live run that
records its verdicts), pair them with human labels, then calibrate. Keep
the judge model and prompt revision pinned; a moving judge silently
re-bases historical scores. Judge costs are separate from candidate
costs and should be recorded under the judge's own model id.

## Limitations

- Human labels are a sample; kappa is sensitive to prevalence and to
  label quality. Report the interval, not just the point estimate.
- A synthetic fixture proves the machinery, never the judge.
- The threshold sweep is a diagnostic, not a licence to pick a
  favourable threshold on the evaluation set.
- Calibration does not alter `judge_lite` behavior. `judge_lite` remains
  a deterministic stand-in; live judges remain explicit opt-ins.

## Exporting real candidates

`scripts/export_calibration.py` reads current runs with manifests and writes a
JSONL worksheet. Human fields are always empty: do not auto-fill them from the
judge.

```sh
python3 scripts/export_calibration.py \
  --store /var/lib/eval-runs \
  --out /tmp/rag-calibration.jsonl \
  --limit 40
```

Then, for every row:

- set `human_label` to `pass` or `revise`;
- set `human_score` from 0 to 10;
- add notes for unsupported claims, irrelevant sources, or missing evidence;
- optionally add A/B pairs if you want position-swap diagnostics.

The export intentionally drops runs without a model/manifest, includes only
content cases, and truncates very long results. It is a worksheet, not a
calibration result. The judge label and score already present are advisory
observations, not human labels.

After labelling:

```sh
eval calibrate /tmp/rag-calibration.jsonl \
  --judge-model "nvidia/nemotron-3-ultra-550b-a55b:free" \
  --prompt-version judge-content-v1 \
  --rubric content-quality/v1 \
  --out calibration/rag-judge.json \
  --markdown calibration/rag-judge.md
```

## Human-labelling checklist

- [ ] At least 30 rows
- [ ] At least 10 A/B pairs for position diagnostics, if available
- [ ] Include both pass and revise examples
- [ ] Include failures and unsupported-claim examples
- [ ] Labels reviewed by a second pass when possible
- [ ] Dataset hash and date recorded with the report
- [ ] Report kept advisory unless the kappa lower bound clears 0.6
