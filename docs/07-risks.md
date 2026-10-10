# G. Risk register

| # | Risk | Likelihood / Impact | Mitigation | Residual |
|---|---|---|---|---|
| R1 | **Architectural overgeneralization** — engine bends toward agents (the author's background) and fits conventional targets poorly | Med / High | Contrast-pair acceptance (deterministic CLI + agent in MVP); "no agent concepts in core" rule enforced in review; A1 thin-adapter test on a non-AI tool | Low |
| R2 | **Protocol incompatibility** — v1 guesses wrong and every adapter breaks on v2 | Med / High | Tolerant-runner/strict-target policy; additive-only minors; conformance suite as a contract; transcripts written before the runner | Low-Med |
| R3 | **Unreliable regression statistics** — flaky gates (erodes trust) or silent misses | Med / High | Suite-level paired bootstrap + δ floor (§8 of brief); synthetic stable/noisy/degraded datasets as permanent tests; per-case output diagnostic-only; no heuristic threshold sold as significance | Low-Med |
| R4 | **Process-management failures** — hangs, zombies (esp. Windows), cross-case contamination, stderr floods | Med / Med | Kill+respawn, timeout rule, capped stderr capture, worker isolation tests incl. Windows CI; worker pool bounded | Low |
| R5 | **Secrets in runs/logs** — keys captured in cassettes, traces, or reports | Low / High | Env allowlisting; fingerprint-only key identity; redaction layer with property tests; `.env` never read wholesale; review checklist item before `--accept`-style baselines | Low |
| R6 | **Scope creep** — server, dashboard, plugins, harness, sandboxing all "just one more milestone" | High / Med | Explicit v1 exclusion list (docs/05); M9+ items each require a demonstrated use case; harness is a future target, never a dependency | Med (discipline-dependent) |
| R7 | **Open-source adoption failure** — builds for an audience of one, docs assume insider context | Med / Low | M8 dogfood rule (outsider integrates from docs alone); tutorial commands executed in CI; compat policy + contribution checks from day one | Low |

Additional noted risks: judge cost/latency (fake-by-default, live opt-in
with caps); Windows-first dev friction for process semantics (test on
both Windows and Linux CI); inventory drift (assumption flags in
`docs/01`, re-verify `digest_agent` offline mode at M1).
