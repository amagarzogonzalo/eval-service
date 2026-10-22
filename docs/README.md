# eval-service documentation

Technical contracts, operating procedures, and the design record for the
`eval-service` platform. Start with the path that matches what you need to do.

> New to the project? Read the [top-level README](../README.md), then
> [`architecture.md`](architecture.md), [`benchmark-method.md`](benchmark-method.md),
> and [`case-study.md`](case-study.md).

## I want to...

| Goal | Read |
|---|---|
| Add a project as an evaluable target | [`integration.md`](integration.md), [`tutorial.md`](tutorial.md) |
| Understand the runtime and trust boundaries | [`architecture.md`](architecture.md), [`security-threat-model.md`](security-threat-model.md) |
| Understand pass rate, latency, sources, and judge scores | [`benchmark-method.md`](benchmark-method.md) |
| Record and aggregate LLM usage | [`llm-events.md`](llm-events.md), [`proxy.md`](proxy.md) |
| Operate the private service | [`runbook.md`](runbook.md), [`observability.md`](observability.md) |
| Run or extend the test suite | [`testing.md`](testing.md) |
| Gate a pull request | [`pr-gate.md`](pr-gate.md), [`quality.md`](quality.md) |
| Deploy or validate the AWS runner | [`10-cicd-aws.md`](10-cicd-aws.md), [`../infra/aws/README.md`](../infra/aws/README.md) |
| Review releases | [`release-checklist.md`](release-checklist.md), [`../CHANGELOG.md`](../CHANGELOG.md) |

## Core concepts

| Document | Contents |
|---|---|
| [`architecture.md`](architecture.md) | Runtime topology, components, one run end to end |
| [`benchmark-method.md`](benchmark-method.md) | Pass rate, latency, sources, judge, current vs legacy |
| [`case-study.md`](case-study.md) | Problem, design, RAG evidence, and limitations |
| [`tutorial.md`](tutorial.md) | Zero-to-gated-suite walkthrough on the demo targets |
| [`run-metrics.md`](run-metrics.md) | `run-manifest/v1`, nullable metrics, compare semantics, retention, dashboard views |

## Integration and protocol

| Document | Contents |
|---|---|
| [`integration.md`](integration.md) | Adapter/suite contract and the `eval/v1` workflow |
| [`03-protocol-v1.md`](03-protocol-v1.md) | Wire protocol history and invariants |
| [`04-domain-model.md`](04-domain-model.md) | Domain types, IDs, records, and stores |
| [`proxy.md`](proxy.md) | Live LLM observation, key injection, budgets, cassettes |
| [`llm-events.md`](llm-events.md) | `llm/v1` usage events and honest metrics |
| [`otel-mapping.md`](otel-mapping.md) | Mapping to GenAI/OpenTelemetry conventions |

## Operations

| Document | Contents |
|---|---|
| [`runbook.md`](runbook.md) | Private access, deploy, schedule, restore, retention |
| [`observability.md`](observability.md) | Phoenix/OTel stack, trace model, ports, retention |
| [`sandbox.md`](sandbox.md) | Docker executor, internal network, proxy-only egress, isolation proof |
| [`watch.md`](watch.md) | Read-only GitHub polling, debounced restart-safe queue, heartbeat |
| [`pr-gate.md`](pr-gate.md) | PR replay gate, provenance, live promotion, narrow-write notifications |
| [`digest-e2e.md`](digest-e2e.md) | Real GitHub + model + PDF + SMTP digest lane |
| [`research-e2e.md`](research-e2e.md) | Research service and live RAG lane |
| [`e2e-contract.md`](e2e-contract.md) | Repeatable live E2E acceptance contract |
| [`oracle-runner.md`](oracle-runner.md) | Oracle Always Free runner, reclamation, backups, ARM64 checklist |
| [`../infra/aws/README.md`](../infra/aws/README.md) | AWS `t4g.small` Terraform runner and Phase 3 evidence |
| [`10-cicd-aws.md`](10-cicd-aws.md) | OIDC plan/drift lane, role boundary, CI/CD design |

## Quality, testing, and security

| Document | Contents |
|---|---|
| [`testing.md`](testing.md) | Test layers, browser E2E, CI jobs, dependency-policy scope |
| [`quality.md`](quality.md) | Rust quality gates, CI matrix, repository audit, known gaps |
| [`security-threat-model.md`](security-threat-model.md) | Assets, controls, and residual risk |
| [`judge-calibration.md`](judge-calibration.md) | Cohen's kappa, A/B swap, thresholds, advisory gating |
| [`release-checklist.md`](release-checklist.md) | Release and evidence checklist |
| [`web.md`](web.md) | Hosted benchmark page and real-data rules |

## Architecture decisions

| ADR | Decision |
|---|---|
| [`adr/0001-eval-protocol-boundary.md`](adr/0001-eval-protocol-boundary.md) | The `eval/v1` protocol boundary |
| [`adr/0002-deterministic-gate-advisory-judge.md`](adr/0002-deterministic-gate-advisory-judge.md) | Deterministic gate, advisory judge |
| [`adr/0003-manifests-and-nullable-metrics.md`](adr/0003-manifests-and-nullable-metrics.md) | Manifests and nullable metrics |
| [`adr/0004-current-vs-legacy-evidence.md`](adr/0004-current-vs-legacy-evidence.md) | Current vs legacy evidence |
| [`adr/0005-free-only-policy.md`](adr/0005-free-only-policy.md) | Free-only policy |

## Historical design record

`01-inventory.md` through `09-recommendation.md` are the original M0–M8 design
and roadmap set. They remain useful context, but the current contracts are
`integration.md`, `proxy.md`, `llm-events.md`, `run-metrics.md`,
`architecture.md`, and the runbooks above.

| Document | Note |
|---|---|
| [`01-inventory.md`](01-inventory.md) | Original project inventory and constraints |
| [`02-architecture.md`](02-architecture.md) | Crate boundaries and data flow |
| [`05-mvp.md`](05-mvp.md) | Original M0–M8 MVP plan (historical) |
| [`06-roadmap.md`](06-roadmap.md) | Historical roadmap |
| [`07-risks.md`](07-risks.md) | Risk register |
| [`08-decisions.md`](08-decisions.md) | Decision log / rationale |
| [`09-recommendation.md`](09-recommendation.md) | Original recommendation |
| [`mvp-digest.md`](mvp-digest.md) | Digest integration MVP notes |
| [`sample-report.md`](sample-report.md) | Example report output |
