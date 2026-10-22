# Documentation index

Start here when you are new to the platform:

| Document | Why read it |
|---|---|
| [`architecture.md`](architecture.md) | Runtime topology, trust boundaries, and one run |
| [`frontend-design.md`](frontend-design.md) | Private UI hierarchy, interaction rules, and accessibility |
| [`case-study.md`](case-study.md) | Problem, design, RAG evidence, and limitations |
| [`benchmark-method.md`](benchmark-method.md) | Pass rate, latency, sources, judge, current/legacy |
| [`runbook.md`](runbook.md) | Private access, deploy, schedule, restore, retention |
| [`security-threat-model.md`](security-threat-model.md) | Assets, controls, and residual risk |
| [`adr/`](adr/) | Architecture decision records |
| [`integration.md`](integration.md) | The adapter/suite contract and the `eval/v1` protocol |
| [`tutorial.md`](tutorial.md) | End-to-end walkthrough on the demo targets |
| [`proxy.md`](proxy.md) | Live LLM observation, budget semantics, cassettes |
| [`llm-events.md`](llm-events.md) | `llm/v1` usage events and honest metrics |
| [`run-metrics.md`](run-metrics.md) | `run-manifest/v1`, metrics, retention, compare semantics, dashboard views |
| [`otel-mapping.md`](otel-mapping.md) | Mapping to GenAI/OpenTelemetry conventions |
| [`watch.md`](watch.md) | Read-only GitHub polling, debounced restart-safe queue, heartbeat |
| [`pr-gate.md`](pr-gate.md) | PR replay gate, provenance, live promotion, narrow-write notifications |
| [`quality.md`](quality.md) | Rust quality gates, CI matrix, repo audit and known gaps |
| [`observability.md`](observability.md) | Phoenix/OTel stack, trace model, ports and retention |
| [`digest-e2e.md`](digest-e2e.md) | Real GitHub + model + PDF + SMTP digest lane and timer |
| [`web.md`](web.md) | Hosted benchmark page and real-data rules |
| [`e2e-contract.md`](e2e-contract.md) | Repeatable live E2E acceptance contract |
| [`judge-calibration.md`](judge-calibration.md) | Cohen's kappa + CI, A/B swap, thresholds, advisory gating |

## Architecture and decisions

| Document | Contents |
|---|---|
| [`01-inventory.md`](01-inventory.md) | Original project inventory and constraints |
| [`02-architecture.md`](02-architecture.md) | Crate boundaries and data flow |
| [`03-protocol-v1.md`](03-protocol-v1.md) | Wire protocol history and invariants |
| [`04-domain-model.md`](04-domain-model.md) | Domain types and stores |
| [`08-decisions.md`](08-decisions.md) | Decision log / rationale |
| [`09-recommendation.md`](09-recommendation.md) | Original recommendation |

## Plans and reports

| Document | Note |
|---|---|
| [`05-mvp.md`](05-mvp.md) | Original M0–M8 MVP plan (historical) |
| [`06-roadmap.md`](06-roadmap.md) | Historical roadmap; current v2 phases are in the runner docs |
| [`07-risks.md`](07-risks.md) | Risk register (still useful) |
| [`mvp-digest.md`](mvp-digest.md) | Digest integration MVP notes |
| [`sample-report.md`](sample-report.md) | Example report output |

## Operations

| Document | Contents |
|---|---|
| [`oracle-runner.md`](oracle-runner.md) | Oracle Always Free plan, reclamation, backups, ARM64 checklist |
| [`../infra/aws/README.md`](../infra/aws/README.md) | AWS `t4g.small` Terraform runner, Phase 3 evidence, runbook |
| [`sandbox.md`](sandbox.md) | Docker executor: internal network, proxy-only egress, isolation proof |
| [`proxy.md`](proxy.md) | `eval proxy` / `eval cassette` operations |
| [`10-cicd-aws.md`](10-cicd-aws.md) | OIDC plan/drift lane, role boundary and CI/CD design |

## Historical plan note

`01`–`09` are the original M0–M8 design/roadmap set. The platform has
since grown a live proxy, cassettes, a portfolio dashboard, and cloud
runner IaC (v2 phases). Treat those numbered documents as historical
design context; the current operational contracts are `integration.md`,
`proxy.md`, `llm-events.md`, `oracle-runner.md`, and the AWS README.
