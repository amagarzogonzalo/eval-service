# Security policy

## Secrets: policy and enforcement

**No credential is ever committed.** This repository is scanned for secrets at
three layers, so an accident has to defeat all of them to land:

| Layer | Tool | When | What it catches |
|---|---|---|---|
| Local pre-commit | [gitleaks](https://github.com/gitleaks/gitleaks) + `detect-private-key` | before the commit exists | the mistake, in milliseconds, offline |
| CI (full history) | gitleaks CLI, `secret-scan` job | every push and PR | anything that bypassed a local hook, plus committed history |
| Platform push protection | GitHub secret scanning | on push | known provider formats, rejected server-side before they land (free on public repos) |

Config lives in `.gitleaks.toml` (default rule set extended with narrow,
documented allowlists only). Run it yourself any time:

```sh
gitleaks git . --config .gitleaks.toml --redact --verbose
```

Enable the local hook once per clone:

```sh
pip install pre-commit
pre-commit install
```

### Allowlist discipline

`.gitleaks.toml` contains exactly one allowlist entry: the deliberately fake
secret shapes in `crates/eval-core/tests/core_tests.rs`, which the redaction
unit test asserts are caught. It is scoped to one file so it cannot mask a real
leak elsewhere. **Never widen an allowlist to silence a finding — rotate the
credential instead.**

## How this repository handles runtime credentials

The platform is designed so secrets only ever live in the environment that
launches an evaluation, never on disk and never in stored results:

- Live model calls are **opt-in** (`EVAL_DIGEST_LIVE=1` / `EVAL_LLM_LIVE=1`).
  With the flag absent or a key missing, the target returns a handled
  `case_error` — it never silently falls back to a network call.
- Credentials arrive only through named environment variables
  (`EVAL_DIGEST_API_KEY`, `EVAL_LLM_API_KEY`). Value names are allowlisted;
  the full parent environment is never dumped to a child process.
- Stored records pass through a redaction layer (`eval-core::redact`) that
  scrubs `Authorization`/Bearer shapes, `key=value` secrets, and `sk-...`
  tokens before anything is written. This is property-tested.
- `.env` files are git-ignored; `.env.example` files, when present, are not.
- Request/cost budgets (`*_MAX_REQUESTS`, timeouts) bound every live run.

See `docs/llm-events.md` for the event contract and
`integrations/README.md` for the per-integration configuration.

### If a secret is ever committed

Order matters — cleaning history first fixes the symptom and leaves the
credential live:

1. **Revoke and rotate** the credential at the provider.
2. Replace it in any environment or CI secret store that used it.
3. Review provider audit logs for use during the exposure window.
4. Only then clean history (`git filter-repo`), and assume forks, clones and
   caches may retain the old value regardless.

## Reporting a vulnerability

Open a private GitHub security advisory on
`amagarzogonzalo/eval-service` (Security → Advisories → Report a vulnerability),
or email the maintainer. Please do not file a public issue for a live
vulnerability. Include reproduction steps and the affected version or commit.

## Scope and honest limitations

- A subprocess is **not a sandbox**. Env allowlisting, working-directory
  confinement, and output redaction are implemented; OS-level isolation
  (containers, seccomp, a separate UID) is required before running untrusted
  targets. The platform does not claim otherwise.
- The local HTTP collector binds to loopback only and supports an optional
  bearer token; it is a convenience inlet, not an authenticated network
  service.
- Statistics are tested against synthetic and seeded fixtures; false-positive
  and false-negative rates on real workloads are not yet calibrated.
