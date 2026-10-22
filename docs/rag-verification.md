# Verify a RAG research answer

This is a human verification procedure for the research lane. The topic
is:

> How retrieval-augmented generation reduces hallucinations in large
> language models

The goal is not to agree with the prose. The goal is to check whether the
answer is supported by the sources the run actually returned.

## Before you start

- Use [`private-access.md`](private-access.md) to open the private UI.
- The RAG topic is `rag-hallucinations` in
  `datasets/research_e2e/topics.json`.
- The adapter default topic is RAG. However, the production service
  suite `integrations/research_agents/live_service.yaml` still hardcodes
  `crispr-offtarget`. Until that suite is updated, a remote production
  run may not be a RAG run. Check the query in the run header before
  verifying; do not assume the topic from the date.
- For a known RAG run, use a local service lane run or a run whose query
  is explicitly the RAG question. A remote run whose query is CRISPR is
  not a RAG verification.

## Open the run

1. Open **Runs** in the private UI.
2. Filter suite `research-agents-service`.
3. Open the latest run with the RAG query. Prefer a current run (no
   legacy badge) with `lane=local-service` or `remote-service`.
4. Confirm the header shows:
   - the RAG query,
   - `model` (`target-service:remote` for remote; a named free model for
     local),
   - `free-only` for local production runs,
   - a trace ID,
   - case count and pass/fail.

API (also pass Caddy Basic Auth with `-u` if using 19090):

```sh
curl -sS -u "$BASIC_USER:$BASIC_PASS" -b /tmp/eval-cookie \
  'http://127.0.0.1:19090/api/run?suite=research-agents-service&ref=research-service-remote&run_id=<run-id>' \
  | jq '{query: .run.query, models: .run.models, content: .run.content, pass: .run.pass}'
```

Expected: the query is the RAG question; `content.nsources` and
`content.citations` are numbers (or `null`/absent, which is `n/a`).

## Read the answer against the sources

Open **Query / Sources / Answer**.

For each of 2–3 representative claims in the answer:

1. Identify the citation marker or the named source.
2. Find that source in the source list; open its DOI/URL.
3. Read the abstract or the relevant passage.
4. Decide:
   - **supported**: the source states the claim (allow paraphrase),
   - **partially supported**: the source is related but the claim is
     stronger or broader,
   - **unsupported**: no source supports it, or the citation points to
     the wrong source.

Record the claim, source identifier, and verdict. Do not use the judge
score as a substitute for this check.

## Verification checklist

- [ ] Query is exactly the RAG hallucination question.
- [ ] Run is current (not legacy) and lane/model are understood.
- [ ] `nsources` is a real count, not `n/a`.
- [ ] `citations` is a real count and citation markers are not empty
      (`[, ]`).
- [ ] At least two claims are checked against source text.
- [ ] Unsupported or padded claims are listed, not hidden.
- [ ] Judge dimensions are reported as advisory, not as proof.
- [ ] Trace ID is recorded with the run ID.
- [ ] A failed check is reported as a failed run, not “flaky” without
      evidence.

## Good and bad examples

Good, local RAG evidence (documented in
[`research-e2e.md`](research-e2e.md)):

```text
Run 1792577496709-e162fb99, local service lane
Artifacts: 16,531 / 14,979 chars; 77 / 67 sources; 9 / 10 citations
Judge: advisory if present; absent is n/a
```

That run proves the local lane can produce a sourced article. It is not a
remote production claim.

Bad, remote artifact (same document):

```text
API returned a wiki/source wrapper plus "## Article"
sources: 0
citation markers: [, ] (empty)
grader: too few citations: 0 / too few sources: 0
```

This must fail verification. Do not count the wrapper as the article and
do not infer missing sources.

Bad, empty citation markers even with a long answer: the answer may read
well but is not grounded; report it as unsupported.

## What to write down

Include in the run note or issue:

- run ID and trace ID,
- lane and model display,
- query,
- `nsources` / `citations` / `artifact_chars`,
- claims checked and verdicts,
- judge overall and dimensions (advisory),
- failures/limitations.

Do not paste provider keys, session cookies, or full page contents into
the note. Source titles/DOIs and short quotes are sufficient.

## Current blocker to a remote RAG claim

Two gaps are visible in the repository, not hidden:

1. `live_service.yaml` still pins `crispr-offtarget`; the production
   workflow therefore cannot be assumed to run RAG.
2. The remote target previously returned a wrapper with zero sources and
   empty citation markers.

Until both are addressed, verify RAG with the local service lane and
label it **local**, or mark the remote RAG run as failed/blocked. Do not
present a CRISPR run as RAG evidence.
