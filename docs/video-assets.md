# Video asset manifest

Do not record the final video until `docs/release-checklist.md` is complete and
the owner declares the freeze. The current numbers are candidates, not final
until the frozen run is recorded.

## Required frozen inputs

- Final commit: `FINAL_COMMIT`
- Final RAG run: `FINAL_RAG_RUN_ID`
- Final digest run: `FINAL_DIGEST_RUN_ID`
- Query: `How retrieval-augmented generation reduces hallucinations in large language models`
- Approved screenshots and screen recording

## Asset list

| ID | Asset | Source | Notes |
|---|---|---|---|
| A1 | Architecture diagram | `docs/architecture.md` | Render fresh at 1920×1080 |
| A2 | Candidate RAG run | private UI → RAG run | Query, sources, answer |
| A3 | Source card interaction | private UI → Sources | Open one DOI; show excerpt |
| A4 | Judge card | private UI → Judge | Score, five dimensions, issues |
| A5 | Benchmark filters | private UI → Benchmarks | Query and LLM/target filters |
| A6 | LLM cards/table | private UI → Benchmarks | Latest judge, pass, p50, sources |
| A7 | Weekly workflow | GitHub Actions | Green digest + research jobs |
| A8 | Restore drill | terminal/runbook | Store validation output, no secrets |
| A9 | Title/end card | editor | “Make agent behavior measurable.” |

## Candidate facts to replace at freeze

| Fact | Candidate value |
|---|---|
| RAG cases | 2/2 |
| Sources | 50 |
| Citations | 7 |
| p50 | 89,029 ms |
| Throughput | 0.87/min |
| RAG judge | 9/10 |
| Digest judge | 6/10 |
| Research model label | `target-service:remote` |
| Digest model label | `gemini:free` |

Verify every value against the frozen run. If a value is unavailable, use
`n/a`; never substitute zero.

## Redaction checklist

- [ ] API token, signing key, and cookies hidden
- [ ] SSH tunnel command and key path hidden
- [ ] AWS account ID, IPv4 address, and bucket path hidden
- [ ] Email addresses and private prompts hidden
- [ ] Browser tabs/bookmarks hidden
- [ ] No `.env` content visible
- [ ] No target repository secrets visible
- [ ] No unverified or synthetic numbers

## Recording plan

1. 1440p or 1080p, 30 fps.
2. Clean browser profile with only the private UI.
3. Zoom 125–150%; keep text legible.
4. Record in this order:
   - overview/current runs
   - RAG run query
   - source card
   - answer section
   - judge card
   - benchmark query filter
   - LLM/target cards
   - weekly workflow
5. Capture still frames for the case study.

## Editing plan

- 90-second primary cut.
- Optional 3-minute technical cut.
- Captions for every number.
- No code scrolling; architecture diagram for 20 seconds maximum.
- End on the benchmark, not on a terminal.

## Handoff message for video agent

Use only the frozen commit and run ID. Do not record from a running production
service without permission. Do not claim judge accuracy, statistical
significance, or zero AWS cost. State that the UI is a private single-user
deployment and that infrastructure is covered by AWS credits.
