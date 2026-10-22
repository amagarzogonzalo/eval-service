# Private UI design and interaction

The private UI is an **evidence console**, not a marketing dashboard. It is
optimized for one operator who needs to verify agent behavior quickly and
record it for a CV.

## Principles

- Evidence before scores: query, sources, answer, checks, then judge.
- One primary number per view; secondary metrics stay quieter.
- Filter state is visible, resettable, and reflected in the row count.
- Tables are scannable: consistent numeric alignment, compact density option,
  and an explicit empty state.
- Charts carry units and accessible data tables; color is never the only signal.
- Current and legacy evidence are never visually conflated.
- No external fonts, analytics, CDNs, or runtime dependencies.

## Information architecture

| View | Primary question |
|---|---|
| Overview | Is the latest current evidence healthy, and what changed? |
| Runs | Which run should I inspect? |
| Benchmarks | How do query/model/target groups compare? |
| Run detail | Can I verify this answer against its sources and judge? |
| Methodology | What do the numbers and `n/a` mean? |

## Interaction rules

- Keyboard focus is visible; navigation exposes `aria-current`.
- Run rows are keyboard-activatable.
- Tabs use `tab`/`tablist` semantics and arrow-key navigation.
- Copy actions are available for run and trace identifiers.
- Loading uses a reserved skeleton area rather than a blank page.
- Reduced-motion preferences disable animation.
- Mobile tables scroll inside a bounded region, not the whole document.

## Data honesty

- `n/a` is a valid, visible value.
- Judge scores are advisory and link to their issues.
- Legacy runs remain inspectable but are excluded from default benchmark charts.
- Aggregate numbers drill down to run-level evidence.

## Verification

- `npm --prefix e2e test` covers login, overview, run evidence, sources, judge,
  benchmark filters, charts, model cards, and legacy toggle.
- `make python-test` covers API/auth/artifact behavior.
- `make verify` remains the full Rust/CI gate.
