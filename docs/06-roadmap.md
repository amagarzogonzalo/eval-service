# F. Implementation roadmap

> Historical note: this is the original M0–M8 plan. The current v2
> operational phases (proxy/cassettes, cloud runner, Docker, watch,
> PR gating, judge calibration) are documented in
> `docs/oracle-runner.md`, `infra/aws/README.md`, `docs/proxy.md`, and
> the `Makefile`. See `docs/README.md` for the index.

Organized by end-to-end capability. Effort in half-days for one
developer; **ranges separate implementation (known) from integration
uncertainty (unknown)**. Out-of-scope items are explicit per milestone.

| M | Milestone + deliverables | Deps | Effort (impl + uncertainty) | Acceptance + tests | Main risks | Out of scope |
|---|---|---|---|---|---|---|
| M0 | **Discovery (this plan).** Inventory, architecture, protocol draft, MVP, roadmap, risks, decisions | — | Done (planning) | Review approval | Inventory drift (mitigated: assumption flags) | Any code |
| M1 | **Protocol + schema.** `eval-protocol` types, generated `eval-protocol-v1.schema.json`, transcripts checked in, conformance fixture set defined | M0 | 2 + 0 | Schema drift check passes; version-mismatch test green | Overgeneralizing v1 (mitigated: deferred list in §C) | Runner, graders |
| M2 | **Minimal runnable evaluator.** `eval-core` types + file store + redaction; `eval-runner` oneshot mode; `eval-cli run` on echo target | M1 | 3 + 1 | E2E: suite YAML → run → stored record, offline | Store layout regrets (mitigated: versioned + migratable) | Worker mode, grades |
| M3 | **Cross-language.** Python + Rust reference shims; conformance suite green for both; `init` scaffolds a new adapter | M2 | 2 + 1 | A5 parity check green; Ruff clean on shim | Shim drift (mitigated: shared conformance) | Real integrations |
| M4 | **Grading + store + baseline.** Config graders, `compare`, human + `--json` reports, exit codes 0/1/2 | M3 | 3 + 1 | A1 thin-adapter test on a toy CLI; A3 repeat-quiet | Grader sprawl (mitigated: promote-after-three rule) | Statistics, judge |
| M5 | **Worker mode + failure handling.** Pool, deadlines, kill/respawn, taxonomy, timeout rule validation | M4 | 2 + 1 | A4 recovery matrix green; no cross-contamination | Zombie processes on Windows (mitigated: explicit kill-tree + test) | Distributed exec |
| M6 | **Regression statistics.** Trials wiring, pass@k unbiased, Wilson CI, paired bootstrap suite gate + δ floor, synthetic stable/noisy/degraded datasets | M5 | 3 + 1 | Gate fires on synthetic degradation, quiet on noise; per-case output is diagnostic-only | Weak-stats temptation (mitigated: §8 method + tests, no heuristic thresholds presented as significance) | Bayesian/sequential methods |
| M7 | **Real integrations.** `cv_toolkit` suite (deterministic) + `digest_agent` fakes suite (sampling + judge-lite); judge calibration set + κ/position/verbosity checks; injection corpus as security suite | M6 | 3 + **3** | A7 two-project demo; A2 intentional-regression demo; A6 offline CI | Offline-drivability of `digest_agent` (fallback: minimal stage pipeline); judge cost (fake by default, live opt-in) | `research-orchestration-agents`, `llms` |
| M8 | **Docs + OSS readiness.** README, architecture doc, protocol docs, tutorial, suite-config reference, CLI docs, exit-code contract, compat policy, contributing + checks, license confirmed | M7 | 2 + 1 | A new user integrates a target from docs alone (dogfood with a reviewer) | Docs-code drift (mitigated: docs tested where possible — tutorial commands run in CI) | Marketing, roadmap promises |
| M9+ | **Advanced, use-case-justified.** SQLite migration (on trigger), `research-orchestration-agents` replay suite, `llms` contract suite, custom-grader subprocess interface hardening, richer stats | M8 | TBD per item | Per-item acceptance | Scope creep (each item needs a demonstrated need) | Server/dashboard, harness |

**Total to OSS-ready MVP (M1–M8): ~20 impl + ~9 uncertainty ≈ 29 half-days
(≈ 15–30% slack included) ≈ 6–8 weeks part-time.** M7 carries the
largest uncertainty (third-party-shaped integration work, not engine work).

Deferred explicitly: server/dashboard/distributed execution, plugin
ABI, SQLite before its trigger, MCP, the optional agent harness
(which becomes a *target*, never a dependency), PDF rendering,
sandboxing beyond the honest boundary.
