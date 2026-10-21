# Threat model

Scope: the private single-user eval-service deployment and the public source
repository. This is a pragmatic model, not a formal audit.

## Assets

- Provider/API tokens and the private-site API token/signing key.
- Run evidence: prompts, outputs, sources, traces, and manifests.
- The self-hosted runner and VM credentials.
- Integrity of grading, pass/fail decisions, and benchmark aggregates.
- Availability of the weekly production run.
- Public trust in the repository and its claims.

## Actors

- Repository visitors and contributors.
- A malicious or compromised target dependency.
- A compromised provider or external target.
- An operator making a mistake.
- Automated scanners and credential harvesters.

## Threats and controls

| Threat | Control | Residual risk |
|---|---|---|
| Secret committed to git | Gitleaks pre-commit + full-history CI + GitHub scanning; env-only runtime | History/forks may retain a leaked value; rotate first |
| Token exposed in UI | HttpOnly signed session cookie; loopback binding; private tunnel | Any local process with the token can log in |
| Unauthorized UI access | Private tunnel plus API token | No multi-user RBAC or rate limiting |
| Malicious target code | Adapter isolation, env allowlist, no target edits; Docker sandbox for untrusted targets | Plain subprocess is not a sandbox |
| Prompt/output leakage | Read-only UI, redaction layer, no public ingress | Stored evidence still contains prompts by design |
| Supply-chain compromise | Locked Cargo, pinned Actions, dependency review, minimal dependencies | No runtime SBOM/provenance attestation yet |
| Grading tampering | Deterministic checks, stored grades, manifest, reviewable run directory | Single operator can still edit local evidence |
| Provider outage/quota | Handled failure classes; free-only policy; no paid fallback | Production run can fail |
| Cost surprise | Budget alerts; documented cost/lifecycle | Billing is delayed; no hard spend cap |
| Evidence loss | S3 mirror and restore drill | Restore has not been automated as continuous replication |

## Access rules

- Keep the UI bound to loopback.
- Never open the private service port publicly.
- Do not put the API token in a URL, screenshot, or recording.
- Rotate tokens on suspected exposure and restart the service.
- Keep `/etc/eval/*.env` mode `0600` and owned by root.

## Public repository boundary

The source repository may be public; the service is not. Do not add live
tokens, private endpoints, or customer data to the repo. Public case studies
should use authorized run evidence and must state when the upstream target
model is unknown.

## Reporting

Use GitHub Security Advisories for vulnerabilities. Do not file a public issue
containing a live secret or exploit chain.
