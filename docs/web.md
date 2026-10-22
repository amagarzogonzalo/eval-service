# Benchmark web views

There are two different views; do not confuse them.

| View | Code | Scope |
|---|---|---|
| Private evidence UI | `web/private_service.py` + `web/private_ui/` | full run store, current and legacy, read-only API |
| Public/static benchmark page | `web/build_benchmarks.py` | latest passing run per suite/ref, no interactive run store |

The private UI is the authoritative operator view. See
[`production.md`](production.md) and [`private-access.md`](private-access.md).

## Private evidence UI

`eval-private.service` serves the static UI and read-only API on
`127.0.0.1:8090`. Caddy on `127.0.0.1:8080` enforces Basic Auth; the
operator reaches it through an SSH tunnel on local port `19090`. It
shows current and legacy runs, benchmark filters/charts, source and
citation counts, judge dimensions, and trace IDs. Authentication is
described in [`private-access.md`](private-access.md).

Do not expose `8090` or Caddy publicly. Do not add write endpoints.

## Public/static benchmark page

`web/build_benchmarks.py` renders a single static page from the canonical
run store. It never invents metrics or examples.

```sh
make web
# or on the VM:
python3 web/build_benchmarks.py --store /var/lib/eval-runs --out /srv/bench-web
```

It selects the latest **passing** run per suite/ref; if the newest
attempt failed, the page keeps the last passing run and labels the failed
attempt. It does not provide the private UI's legacy toggle or run
explorer.

Suites without `SUITE_INFO` entries still render using a generic
description, so new integrations are not silently dropped.

## Hosting

The public page is static files. Host them on a listener that is
**separate** from the private Caddy service on `8080` (for example,
another Caddy site on `8081` or an object-store bucket). Do not reuse the
private listener and do not expose the run store itself.

The canonical evidence is the run store/S3; both pages are read-only
views. See [`benchmark-evidence.md`](benchmark-evidence.md) for current
vs legacy and comparability rules.

## Adding a benchmark

- Add the suite to `SUITE_INFO` for a human title and use cases.
- Keep relevant real run data in the store/S3 before publishing.
- Do not add synthetic examples as if they were real.
- Regenerate the page after new runs.
