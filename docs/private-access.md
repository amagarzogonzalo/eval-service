# Private access and authentication

The production UI is private. It is not a public website and it has no
anonymous read path. Access requires an SSH tunnel plus two independent
credentials: Caddy Basic Auth and the application API token.

## Port map

| Port | Process | Purpose |
|---|---|---|
| `127.0.0.1:8090` | `eval-private.service` | read-only API + static UI (loopback only) |
| `127.0.0.1:8080` | Caddy | Basic Auth in front of `8090` |
| `localhost:19090` | your SSH client | tunnel endpoint on the workstation |

The application listens on `8090`; operators normally tunnel to Caddy on
`8080` so Basic Auth is enforced. Tunneling directly to `8090` bypasses
Basic Auth and leaves only the API token — do not use that path except
for a short local diagnostic.

## Open the tunnel

```sh
ssh -i ~/.ssh/eval-runner-aws -N \
  -L 19090:127.0.0.1:8080 \
  ubuntu@13.60.60.51
```

Keep the session open and browse to <http://127.0.0.1:19090>. Expected:
a Caddy Basic Auth prompt, then the eval-service token login page.

If Caddy is unavailable, the application itself is still on the VM at
`127.0.0.1:8090`; diagnose it with
`curl -sS http://127.0.0.1:8090/healthz`. Do not expose either port on a
public interface.

## Two-layer authentication

```text
browser ──Basic Auth──► Caddy :8080 ──proxy──► eval-private :8090
                                                    │
                                    POST /session (API token)
                                                    ▼
                                    signed HttpOnly session cookie
                                    (12 h, SameSite=Strict)
```

1. **Caddy Basic Auth** uses `EVAL_BASIC_AUTH_USER` and
   `EVAL_BASIC_AUTH_HASH` from `/etc/eval/private-site.env`.
2. **Application login** exchanges `EVAL_SITE_API_TOKEN` for a signed
   session cookie (`EVAL_SITE_SIGNING_KEY`). The cookie is HttpOnly,
   SameSite=Strict, and expires after 12 hours.
3. `/api/*` requires the session cookie. `/healthz` is unauthenticated
   but bound to loopback; it returns only `{"ok": true}`.

There are no write endpoints. Logout clears the cookie at `/logout`.

## Credential handling

All secrets live in `/etc/eval/private-site.env`, created with `umask
077` (expected mode `600`, root-owned). Verify with
`sudo stat -c '%a %U:%G' /etc/eval/private-site.env`. Do not print,
copy, paste into chat, or commit them:

- `EVAL_SITE_API_TOKEN`
- `EVAL_SITE_SIGNING_KEY`
- `EVAL_BASIC_AUTH_USER`
- `EVAL_BASIC_AUTH_HASH`

When entering the API token, prefer a password manager or a terminal
prompt. Avoid `echo` and shell history:

```sh
read -rsp 'Caddy user: ' BASIC_USER; printf '\n'
read -rsp 'Caddy password: ' BASIC_PASS; printf '\n'
read -rsp 'API token: ' TOKEN; printf '\n'
curl -sS -u "$BASIC_USER:$BASIC_PASS" -c /tmp/eval-cookie -o /dev/null \
  -d "token=$TOKEN" http://127.0.0.1:19090/session
unset BASIC_USER BASIC_PASS TOKEN
```

The `curl` request writes the session cookie to `/tmp/eval-cookie`, not
the token. Delete the cookie file when finished. The browser flow is
easier; this is for scripted checks.

## Verify access

```sh
# From the VM: app health without auth (loopback only)
curl -sS http://127.0.0.1:8090/healthz
# expected: {"ok": true}

# From the workstation through the tunnel: Caddy + app
curl -sS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:19090/login
# expected: 200

# Unit state
sudo systemctl is-active eval-private.service eval-caddy.service
```

In the UI:

- Overview: current runs, latest per suite, judge/source chips.
- Runs: search/filter all runs; legacy badge visible.
- Benchmark: current/legacy filters, model/query filters, charts.
- Run detail: query, sources, answer, checks, judge, logs, files.

## Rotation

Rotate the API token and signing key together; rotating only the token
does not invalidate existing signed sessions.

```sh
sudo sh -c 'umask 077
tmp=$(mktemp)
printf "EVAL_SITE_API_TOKEN=%s\n" "$(openssl rand -hex 32)" > "$tmp"
printf "EVAL_SITE_SIGNING_KEY=%s\n" "$(openssl rand -hex 32)" >> "$tmp"
grep -E "^EVAL_BASIC_AUTH_(USER|HASH)=" /etc/eval/private-site.env \
  >> "$tmp" || true
mv "$tmp" /etc/eval/private-site.env'
sudo systemctl restart eval-private.service
```

Then re-enter the new API token on the login page. Do not print the
generated values. If Basic Auth must also rotate, update
`EVAL_BASIC_AUTH_HASH` with `caddy hash-password` and restart Caddy.

## Failure modes

| Symptom | Likely cause | Action |
|---|---|---|
| Connection refused on 19090 | tunnel closed or Caddy down | reopen tunnel; `systemctl status eval-caddy` |
| Basic Auth prompt loops | wrong user/hash | re-check env file; restart Caddy |
| `Invalid token` | wrong API token | re-read token with `sudo`, do not paste into logs |
| UI loads but API 401 | expired 12 h session | log in again |
| 404 on a run | wrong suite/ref/run id | reopen from Runs; paths are exact |
| `/healthz` OK but no data | store path/permissions | check `ReadOnlyPaths` and store ownership |
| 403/redirect on static file | UI not copied to `/srv/private-ui` | redeploy `web/private_ui` |

See [`troubleshooting.md`](troubleshooting.md) for service-level
recovery.

## Do not

- Do not bind `8090` or Caddy to `0.0.0.0` for convenience.
- Do not add write endpoints or a public unauthenticated health page.
- Do not put tokens in URLs, screenshots, tickets, or docs.
