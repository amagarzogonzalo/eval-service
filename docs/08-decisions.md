# H. Key unresolved decisions

Only decisions that materially affect architecture or implementation.
Each has options, trade-offs, evidence, and a recommended default.
Confirm or override in review.

## H1. Crate count: 5 vs 7
- **Options:** (a) 5 crates (store→core, compare→graders), (b) 7 as
  briefed.
- **Evidence:** file store ≈ 200 lines; comparison is meaningless
  without grades (same versioning lifecycle).
- **Recommendation: (a) 5.** Split on demonstrated strain, not upfront.

## H2. Persistence: files v1, SQLite later?
- **Options:** (a) files now, migrate on trigger; (b) SQLite from day one.
- **Evidence:** expected volume is tens of runs, single writer, local
  use; files are diffable and need zero setup.
- **Recommendation: (a).** Migration trigger: >10k runs, concurrent
  writers, or real ad-hoc query need — whichever comes first.

## H3. Statistics engine: hand-rolled vs `statrs`
- **Options:** (a) hand-rolled Wilson/bootstrap/pass@k with reference-
  value unit tests; (b) `statrs` dependency.
- **Evidence:** three small, exactly-testable functions; dependency
  cost (API surface, MSRV, audit) exceeds implementation cost.
- **Recommendation: (a) for v1.** Revisit only if Bayesian or
  sequential methods prove better on measured gate performance.

## H4. Judge model pinning
- **Options:** (a) pin judge model in suite config, independent of any
  target's model; (b) follow latest.
- **Evidence:** your `harness.toml` already pins the judge separately
  because a moving judge silently re-bases every historical score.
- **Recommendation: (a).** Plus calibration gate (κ ≥ 0.6 with CI,
  position/verbosity checks) before any verdict counts.

## H5. License for open-source release
- **Options:** MIT / Apache-2.0 / dual.
- **Evidence (fact):** `digest_agent` and `cv_toolkit` are MIT.
  `research-orchestration-agents` and `llms` licenses **not verified**.
- **Recommendation: MIT** (consistency + simplicity), **conditional**
  on confirming the two unverified licenses impose no conflict.

## H6. MSRV
- **Options:** (a) 1.85 floor (edition 2024 minimum); (b) latest-stable-minus-N rolling.
- **Recommendation: (a)** declared in the workspace manifest and
  checked in CI. Revisit if a needed dependency requires newer.

## H7. Agent harness relationship
- **Options:** (a) harness stays a separate optional component and a
  future *target*; (b) co-build.
- **Recommendation: (a)** per brief §9. No harness code until the
  evaluator demonstrates value on both integration candidates.

## H8. Digest delivery scope (carried from the Rust-harness thread)
- **Options:** (a) markdown + styled HTML in any future digest work,
  PDF deferred; (b) full PDF parity.
- **Recommendation: (a).** The value is loop/evals/gate, not
  rendering; HTML→PDF via headless browser if parity is ever needed.

## H9. Digest replace-vs-parallel (carried, still open)
- Still unanswered. **Recommendation: run alongside** (head-to-head
  comparison is the strongest demonstration of a cross-language gate;
  the proven Python cron keeps running until beaten). Needed before
  any digest-implementation milestone — not before the eval platform.
