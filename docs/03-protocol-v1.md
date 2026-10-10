# C. Protocol v1 draft (`eval/v1`)

Design goals: minimal, versioned, debuggable with `cat`, strict where
it protects the runner, tolerant where it protects the future.
Nothing here assumes agent traces, tokens, or cost — AI metrics are
optional fields, never required.

## Transport

- Subprocess. Runner writes to target stdin, reads target stdout.
- **stdout: JSONL only**, one message per line. Anything else on
  stdout is a protocol violation.
- **stderr: free-form logs**, never parsed. Targets log there.
- UTF-8, `\n`-terminated lines. Max line 8 MiB (larger → `ProtocolError`).

## Modes and lifecycle

Runner sends the handshake as the first stdin line:

```json
{ "protocol": "eval/v1", "mode": "worker" }
```

- **`worker` (default):** persistent process, N cases per connection.
  Target replies `{"protocol":"eval/v1","ready":true}`, then the
  runner streams `case` messages and the target answers each with
  events + one terminal message. Runner ends with
  `{"type":"shutdown"}`; target exits 0.
- **`oneshot` (simple mode, kept deliberately):** for stateless
  targets that cannot loop. Sequence: handshake → exactly one `case`
  → events → one terminal message → target exits 0. No `shutdown`.

Version mismatch (any other major version) → runner aborts with a
`ProtocolError` naming expected vs received. This is a **runtime**
deserialization check, not a compile-time claim.

## Messages

```jsonc
// runner → target: evaluation request
{ "type": "case", "case_id": "cv-eval-03",
  "input": { "argv": ["eval", "output/build/cv_ink.pdf"], "stdin_text": "" },
  "limits": { "timeout_ms": 30000, "max_output_bytes": 1048576 },
  "env": { "OFFLINE": "1" } }
// target → runner: progress/diagnostic (0..n, ordered by seq)
{ "type": "event", "case_id": "cv-eval-03", "seq": 0, "name": "stage_done",
  "data": { "stage": "extract" } }
// target → runner: terminal success. NOTE: no trace array — the streamed
// events ARE the trace; the runner assembles them. (No duplication.)
{ "type": "result", "case_id": "cv-eval-03", "ok": true,
  "output": { "text": "score=71.2", "files": ["output/reports/cv_summary.json"] },
  "metrics": { "wall_ms": 4120 },              // optional block; absent ⇒ n/a, never zero-filled
  "ai": { "model": "qwen3:4b", "tokens_in": 1200, "tokens_out": 300, "cost_usd": 0.0 }  // optional
}
// target → runner: handled negative outcome (target behaved correctly,
// the answer is "no"). Exit code stays 0.
{ "type": "case_error", "case_id": "cv-eval-03", "code": "no_sources",
  "message": "fixture directory empty", "retriable": false }
// runner → target (worker mode only)
{ "type": "shutdown" }
```

Generic `input` shape (not agent-specific): `argv`, `stdin_text`,
plus an open `params` object for structured inputs. `output` mirrors
it: `text`, `files` (artifact references = relative paths the runner
records but does not fetch), open `data` object.

## Timeouts, cancellation, crashes

| Situation | Target behavior | Runner behavior |
|---|---|---|
| Handled failure | `case_error` line, exit 0 at shutdown | graded failure with code |
| Deadline exceeded | — (no cooperation expected) | kill, respawn worker, record `Timeout` |
| Crash (nonzero exit) before terminal message | process dies | `Crash` + stderr tail (capped) |
| Malformed line / unknown protocol | — | `ProtocolError`, kill + respawn |
| Schema violation in a well-formed line | — | `ProtocolError` |

**Timeout rule:** the runner validates at suite load that
`limits.timeout_ms` ≥ 1.5× the target's own internal wall budget
(where the target declares one), so a graceful budget-exhaustion and
a runner kill are distinguishable by construction.

## Ordering, correlation, compatibility

- Every target message carries `case_id`; events carry `seq`
  (0-based, gapless per case). The runner rejects out-of-order or
  cross-case lines as `ProtocolError`.
- **Runner side: tolerant** of unknown fields (forward compatible).
  **Target side: strict** (`deny_unknown_fields` on handshake/case).
- Minor protocol additions must be backward-tolerant; any breaking
  change becomes `eval/v2` with an explicit negotiation path.

## Conformance testing

Permanent fixtures every target (including both reference shims) must
survive: echo (happy path), hang (→ `Timeout`), crash (→ `Crash`),
malformed output (→ `ProtocolError`), wrong-version handshake
(→ clean reject), unknown-field case (tolerated). Conformance is a
release gate for new adapters.

## Security boundary (honest)

A subprocess is **not a sandbox**: it inherits the runner's user,
filesystem, and network unless restricted. v1 provides: env
allowlisting (runner passes only declared variables), working-directory
confinement, no secret capture (redaction in `eval-core`), and a
documented statement that OS-level isolation (containers, seccomp,
separate UID) is required before running untrusted targets. The plan
does not promise sandboxing it does not implement.

## Stable in v1 vs deferred

- **Stable:** handshake, both modes, six message types, failure
  taxonomy, timeout rule, correlation, compatibility policy,
  conformance set, security statement.
- **Deferred:** artifact fetching (paths only in v1), streaming
  backpressure, multi-target fan-out in one worker, protocol-level
  auth (local-first needs none), binary payloads.
