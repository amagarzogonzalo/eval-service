# D. Domain and result model

## Entities

- **Suite** (`suite.yaml`, versioned): id, version, project ref,
  list of cases, grader bindings, gate policy ref, dataset version.
- **Case**: id, tags, `input` (argv/stdin/params), `limits`,
  expected properties (grader configs), `trials` (1 deterministic,
  ≥5 sampling), deterministic flag.
- **Trial**: one execution of a case (trial 0..n).
- **Observation** (raw): terminal message verbatim, events verbatim,
  exit code, wall time measured runner-side, stderr tail (capped,
  redacted).
- **Grade** (derived): per-grader pass/fail + score + evidence pointer.
- **RunRecord**: suite + code version (git sha or `dirty`), dataset +
  grader versions, trials with observations, grades, gate decision.
- **Baseline**: pointer to a RunRecord (`baselines/<suite>.json`),
  never a copy.

## Result schema (required / optional / n/a)

Required on every trial: `suite_id`, `suite_version`, `case_id`,
`trial`, `status` (ok / case_error / timeout / crash /
protocol_error), `wall_ms` (runner-measured), `code_version`.
Optional: `output`, `events`, `metrics.*`, `ai.*`,
`artifacts` (paths only). Inapplicable data is **absent**
(`Option::None`), never zero — a missing token count must not read
as "free", and a missing latency must not read as "fast".

## Raw vs derived separation

`observations/` (immutable once written) and `grades/` +
`decision.json` (recomputable) live side by side in the run
directory. Changing a grader or threshold re-grades stored
observations without re-executing anything. This is what makes
"evaluation rules can change without losing evidence" true.

## Secrets and payloads

- The runner passes only allowlisted env vars; it never dumps the
  environment. Stored records keep **key fingerprints**
  (`blake3(key)[:8]` + provider name), never key material.
- `RedactingTraceSink`-equivalent filtering runs on every stored
  string (secret patterns, `Authorization` headers, `*_KEY=*`
  shapes). Redaction has property tests.
- Large payloads: store paths + content hashes, not copies, beyond a
  1 MiB cap per run directory (configurable).

## Versioning and migration

Stored runs carry `schema_version`. Migrations are forward-only
converters (`migrate_N_to_N+1`) applied on read; old runs are never
rewritten. A021-style rule: additive changes are minor; removing or
retyping a required field is a major version with a documented
converter or an explicit break notice.

## Grader model

- **Config-driven** for the common checks: `exit_code`,
  `exact`, `regex`, `json_schema`, `range`, `stability`
  (same case twice ⇒ same output, for deterministic targets),
  `file_exists` / `file_matches`.
- **Custom graders** via a documented subprocess interface
  (same JSONL discipline, `grade` request type) — no plugin ABI in
  v1, deliberately. A plugin system before three real custom graders
  exist would be premature abstraction.
- **Boundary:** shared mechanics (parsing, comparison, statistics)
  live in the platform; project-specific judgments (rubrics,
  thresholds, semantic quality) live in suite config + custom
  graders. If a judgment appears in three suites, promote it to a
  shared grader.

## Universal vs domain layers

Universal (always available): process status, timeouts/limits,
output assertions, schema validation, invariants, latency where
measurable. Domain (opt-in per suite): LLM semantic quality
(judge + calibration), agent task/tool behavior, data-transformation
invariants, API contracts, automation side effects, security
(canary-based injection checks). Domain graders declare their
requirements (e.g. "needs judge model"); the runner skips them with
`n/a` — not failure — when requirements are unmet.
