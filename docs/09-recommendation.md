# I. Final recommendation

## Smallest implementation that proves usefulness

Build M1–M5 (protocol → minimal runner → shims → graders/store →
worker/failures), then M7's `cv_toolkit` suite first: it is
deterministic, local, and cheap, so the engine, store, compare, and
gate are all proven before any nondeterminism enters. Add the
`digest_agent` fakes suite second — that is the moment the platform
proves the cross-language, bounded-nondeterminism, judge-graded half
of its thesis. Stop and demo at that point; everything else is M9+.

Concretely, the proof is the acceptance battery in `docs/05-mvp.md`
(A1–A7): thin-adapter with zero engine diff, intentional regression
flipping the gate with an explanation, quiet re-runs, recovery
matrix, shim parity, offline CI, and the two-project report.

## What to build next (in order)

1. M6 statistics hardening — only after real suites exist to tune
   against; synthetic datasets first, then measured false-positive /
   false-negative rates on the two candidate suites.
2. `research-orchestration-agents` replay-only suite — highest-value
   third integration (reuses its cassettes; exercises the platform on
   a large real surface).
3. Custom-grader subprocess interface hardening — after three real
   custom graders exist to generalize from.

## What stays out of scope

Server, dashboard, distributed execution, plugin ABI, SQLite before
its trigger, MCP, the agent harness as a dependency, PDF rendering,
live-provider evals in the default path, and any sandboxing claim
beyond subprocess + env allowlisting + documented OS-level guidance.

## Success measure (from the brief, adopted verbatim)

Not feature or crate count: **whether one maintainable Rust engine
reliably evaluates different existing projects through a small common
interface, detects meaningful regressions, explains its decisions,
and remains easy to extend.** The A1–A7 battery is the operational
form of that sentence — if it passes, ship v1; if it doesn't, the
failing test names the next milestone.
