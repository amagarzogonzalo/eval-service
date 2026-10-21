# Private eval-service web

Read-only evidence UI for one operator. The service binds to loopback and uses
a separate API token exchanged for an HttpOnly session cookie. There is no
public ingress in the current deployment.

## Layout

- `eval-private.service` — private UI/API process on `127.0.0.1:8090`
- `/etc/eval/private-site.env` — API token and cookie signing key (mode 0600)
- `/srv/private-ui` — deployed static UI
- `/var/lib/eval-runs` — canonical run store
- Optional `Caddyfile` / `eval-caddy.service` — only if an operator chooses a
  local TLS reverse proxy; not part of the current access path

## Access

Use an SSH tunnel. The token is never printed by the service.

```sh
ssh -N -o ExitOnForwardFailure=yes \
  -i "$EVAL_VM_SSH_KEY" \
  -L 19090:127.0.0.1:8090 \
  "$EVAL_VM_USER@$EVAL_VM_HOST"
```

Open `http://localhost:19090/` and enter the site API token from
`/etc/eval/private-site.env`.

## Service

```sh
sudo systemctl status eval-private
sudo systemctl restart eval-private
sudo journalctl -u eval-private -n 100 --no-pager
curl -fsS http://127.0.0.1:8090/healthz
```

Deploy static files separately from the Python process:

```sh
sudo cp web/private_ui/index.html web/private_ui/app.js web/private_ui/styles.css /srv/private-ui/
sudo chown eval:eval /srv/private-ui/index.html /srv/private-ui/app.js /srv/private-ui/styles.css
sudo systemctl restart eval-private
```

## Security

- Do not expose port 8090 publicly.
- Do not place the API token in a URL, screenshot, or recording.
- Rotate the token and signing key after suspected exposure; restart invalidates
  existing sessions.
- Keep `/etc/eval/private-site.env` root-owned and `0600`.
- Use [the production runbook](../../docs/runbook.md) for recovery, retention,
  S3 lifecycle, and restore drills.

## Optional Cloudflare Tunnel

If a stable private hostname is required, a Cloudflare Tunnel can terminate
outside the VM. Put access control in front of it; the API token remains a
second factor. Never publish the service without authentication.
