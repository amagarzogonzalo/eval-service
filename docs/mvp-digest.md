# MVP scope: digest-agent evaluation (Loop 1)

## Workflow demonstrated end to end

1. Select project + suite: `eval-workspace.toml` → `integrations/digest_agent/suite.yaml`.
2. Execute representative cases through the real LangGraph orchestrator
   offline (project's own fakes; zero repo changes).
3. Capture outputs (digest summary + full state JSON), exit status,
   and failure evidence.
4. Collect genuine usage via the port decorator: per-call latency,
   outcome, errors, call counts, task association. Token counts stay
   unavailable — the provider port returns a bare string and the HTTP
   layer never reads `usage` blocks (verified by grep: the word
   "usage" appears nowhere in `digest_agent/.../adapters/driven/`).
   No proxy: provider endpoints are hardcoded class attributes with
   no env override, so endpoint redirection without source changes is
   impossible; runtime patching is out of scope.
5. Task-level latency from measured call spans + end-to-end wall time.
6. Grade with explicit criteria (exit code, regex, range, stability).
7. Persist run records + usage events with provenance (origin +
   context labels).
8. Compare candidate vs baseline; gate with evidence.
9. Show quality, usage, cost (where calculable), performance in the
   existing dashboard (usage views read llm/v1 events only; legacy
   ai blocks never summed).

## Scenarios (all against real orchestration code)

| # | Scenario | Driver input | Expected observable |
|---|---|---|---|
| 1 | Empty window | no activity | "Nothing to report today", outcome success, bounded attempts |
| 2 | Normal task | 1 merged PR | PR number + files in summary, outcome success |
| 3 | Weak item | fake completions score 2/10 | review loop iterates, `review_count ≤ max_reviews`, terminates (accept or give_up) |
| 4 | Upstream failure | activity source raises | `outcome: error`, handled failure branch, no crash |

## Metrics genuinely available

Model id per call, per-call wall latency, outcome/error codes, call
counts, retries (attempt counts), task span, review counts/scores,
end-to-end wall time. Unavailable: token counts, TTFT/streaming,
provider-reported cost (estimated cost only where pricing covers the
model, labeled estimated).

## Acceptance

- All four scenarios run green through `eval run` with exit 0.
- Each negative control (crash/hang/malformed/timeout injected at the
  shim level, weak scores and fetch failures at the driver level)
  lands in its own taxonomy slot with evidence.
- Usage events import cleanly; aggregates match independent recomputation.
- Dashboard comparison renders quality + usage side by side.
- Full workspace suite stays green; protocol untouched.
