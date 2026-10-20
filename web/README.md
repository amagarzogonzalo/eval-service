# Benchmark web UI

Static generator for the hosted benchmark page.

```bash
make web
python3 web/build_benchmarks.py --store /var/lib/eval-runs --out /srv/bench-web
```

It reads `metadata.json`, `grades.json`, `decision.json` and optional
`trace.json` from the canonical store. Missing metrics are shown as
missing; no values are estimated.
