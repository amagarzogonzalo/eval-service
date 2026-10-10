# A. Repository inventory

Inspected 2026-10-09 by reading repo roots, manifests, test layouts,
eval tooling, and CI config. Paths are relative to `../` (the Workspace).
**Assumption:** file states reflect the working tree, not a clean checkout.

## Summary table

| Project | Purpose | Technology | Evaluation maturity | Integration approach | Initial test cases | Difficulty | Reuse potential |
|---|---|---|---|---|---|---|---|
| `digest_agent/` | Daily GitHub digest: PRs/diffs → review committee → PDF email (`README.md:1-7`) | Python 3.11+, setuptools, LangGraph; hex ports/core/adapters (`src/digest_agent/`) | **Medium-high.** 8 pytest files (`tests/`); `core/fakes.py`, `deterministic.py`, `policy.py`; CI `ci.yml` + `daily-digest.yml` cron; no eval gate | Subprocess adapter driving the orchestrator through its fakes/offline path; judge-graded summaries + deterministic invariants (termination ≤ `REVIEW_MAX_REVIEWS`, citations present) | Empty activity window → empty digest; weak summary re-queued within budget; loop terminates ≤ max reviews; canary token never leaks into PDF | **Medium.** Offline drivability via `core/fakes.py` is **assumed** — verify at M1, fallback is a real minimal stage pipeline over fixtures | Ports/adapters pattern (proves thin-adapter thesis); fallback-provider chain as a reliability case study |
| `other/cv_toolkit/` | CV toolkit: YAML resume → multi-theme PDFs + hybrid ATS scoring (`README.md:1-12`) | Python 3.11+, setuptools, console script `cv = evaluation.cli:main` (`pyproject.toml:28-29`); rendercv, sentence-transformers, sklearn | **Medium.** 4 pytest files (`tests/test_cli.py`, `test_scorer.py`, `test_batch.py`, `test_compiler.py`); deterministic-by-default scorer, LLM opt-in | Subprocess adapter around the `cv` CLI: `validate`/`eval` commands with JSON outputs (`output/reports/cv_summary.json`); exact + range graders | `validate` rejects bad YAML (exit ≠ 0); `eval` score in [0,100] and stable across reruns; batch run folder naming convention holds | **Low.** Deterministic, local-first, machine-readable outputs, MIT licensed | Deterministic-grader patterns (ranges, stability); CLI-adapter reference implementation |
| `research-orchestration-agents/` | Zero-cost deep-research library: federated search + LangGraph + file persistence (`README.md:6-10`) | Python 3.11–3.13, hatchling; hex core + LangGraph orchestration; OTel `gen_ai.*` metrics (`src/knowledge_engine/observability/telemetry.py`) | **High.** `tests/{unit,integration,e2e,fixtures}`; `evals/` with YAML cases (`research`, `deep`, `biomed`, `web`, `_demo`), cassettes, snapshots, `lib/{cassette,invariants,digest,judge}.py`; gate `make check` = lint+types+arch+semgrep+agents+context+test+evals (`Makefile:52`); workflows `ci/ci-full/evals-drift/harness/research-live` | Phase-2 target: adapter over recorded cassettes first (replay-only suite), live-record later; invariant + judge graders map 1:1 to platform grader layers | Cassette replay bit-identical; dropped citation → BREAKING diff; budget overflow fails gate | **Medium-high (deferred).** Richest surface but largest; integrate after the platform proves itself on the two candidates | **Highest reuse:** cassette record/replay + wipe-before-record discipline (`evals/README.md:36-50`); invariant/digest/judge layering; `samples`+`pass_at_k`; judge calibration; budget-as-gate; evals-drift CI pattern |
| `harness-skeleton/` | Provider-agnostic agent-harness template: profiles, context budget, eval module (`README.md:1-12`) | Python (scripts) + TOML/JSON config; `bin/{apply_profile,context_budget,sessions,lint_agents}.py`; `evals/runner.py` + `lib/{cassette,digest,invariants,judge}.py` | **Medium (as a template).** `evals/cases/_demo.yaml`, snapshots, calibration dir; `harness.toml` pins `judge_model` separately from active profile | Do not integrate as a *target*; mine as the **design donor** for runner semantics (replay→assert→diff→judge→report, `NONDETERMINISM` check, `--accept` human-only snapshots) | n/a (template, not a product) | n/a | Runner control flow; judge-must-not-follow-profile rule; context-budget-as-CI-gate idea |
| `llms/` (AI Factory) | Local inference console: FastAPI `:8090` + React `:3000`, vLLM/Ollama/HF stacks, MLflow, training recipes (`README.md:1-24`) | Python 3.12 backend (`ai-infra-lab-server`: FastAPI, uvicorn, httpx, MLflow) + TypeScript frontend; Docker Compose GPU stacks; pytest + respx; `test_hexagonal_boundaries.py`; pinned recipe `datasets/recipes/pandas-codealpaca.yaml` (revision `798c567` + manifest checksum) | **Medium.** `backend/tests/{unit,integration}` incl. contract tests (`test_compose_contracts.py`); `make verify/validate/doctor`; heavy external deps (Docker, GPU, network) | Phase-2+: adapter over config/contract surface first (compose validation, catalog, recipe-manifest checks); live-service cases only with an explicit `services` marker and Docker present | Recipe manifest checksum matches materialized JSONL; `docker compose config` valid; API contract tests offline via respx | **High (deferred).** GPU/Docker/network coupling; evaluate the testable seams, not the running stack, in v1 | Hexagonal-boundary test pattern; pinned-recipe + manifest provenance pattern; contract-test style |

## Cross-cutting findings

1. **Two projects already implement the platform's core ideas in Python.**
   `research-orchestration-agents/evals/` and `harness-skeleton/evals/`
   both do cassette record/replay, structural digests, calibrated judges,
   and CI gating. The Rust platform standardizes — not invents — this.
2. **Deterministic core + bounded nondeterminism is the norm.**
   `digest_agent` bounds its LLM loop (`REVIEW_MAX_REVIEWS`, `REVIEW_MIN_SCORE`
   in `.env.example`); `cv_toolkit` is deterministic unless `--llm`.
   The platform's staged model (deterministic first, statistics later)
   matches what actually exists.
3. **Machine-readable outputs already exist where integration is cheapest.**
   `cv_toolkit` emits `cv_summary.json`; `digest_agent` has a
   deterministic snapshot; `llms` has contract tests. Adapters wrap
   outputs — no rewrites needed.
4. **Secrets live in env files everywhere** (`.env.example` in
   `digest_agent/`, `llms/`, `research-orchestration-agents/`; `.env`
   is git-ignored). The platform must never capture env contents —
   only key fingerprints (see `docs/04-domain-model.md`).
5. **Licenses (fact, checked):** `digest_agent/LICENSE` and
   `cv_toolkit` `pyproject.toml` are MIT. `research-orchestration-agents`
   and `llms` licenses were **not verified** — confirm before
   open-source release (see `docs/08-decisions.md`).

## Initial integration candidates (contrast pair)

1. **AI-enabled: `digest_agent/` via fakes (medium).** Exercises events,
   bounded nondeterminism, judge grading, budgets, and the injection
   corpus. Assumption to verify first: the orchestrator runs end-to-end
   offline through `core/fakes.py`.
2. **Conventional: `other/cv_toolkit/` CLI (low).** Exercises process
   exit codes, JSON-schema validation, determinism/stability checks, and
   latency measurement. No network, no keys, no GPU.

`research-orchestration-agents` (replay-only suite) is the designated
third integration. `llms` stays deferred until services can be marked
optional.
