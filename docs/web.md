# Benchmark website

`web/build_benchmarks.py` renders a single static page from the canonical
run store. It never invents metrics or examples.

## Build

```bash
make web
# or on the VM:
python3 web/build_benchmarks.py --store /var/lib/eval-runs --out /srv/bench-web
```

The page shows, per suite and latest ref:

- benchmark purpose and real use cases,
- pass/fail case counts,
- trial counts and p50 case latency,
- positive and negative example cases with grader evidence,
- S3 artifact path,
- trace ID when `trace.json` exists.

Suites without `SUITE_INFO` entries still render using a generic
description, so B/C additions are not silently dropped.

## Host

Caddy serves `/srv/bench-web` on `127.0.0.1:8080`. Access through the
same SSH tunnel described in `docs/observability.md`:

<http://127.0.0.1:8080>

The page links to S3 and to trace IDs in Phoenix. The canonical evidence
is the run store/S3; the page is a read-only view.

## Adding a benchmark

- Add the suite to `SUITE_INFO` for a human title and use cases.
- Keep relevant real run data in the store/S3 before publishing.
- Do not add synthetic examples as if they were real.
- Regenerate the page after new runs.
