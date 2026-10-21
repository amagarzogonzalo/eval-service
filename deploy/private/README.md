# Private eval-service web

Read-only service for one user. Caddy terminates TLS and enforces Basic
Auth; `web/private_service.py` requires a separate API token and stores
it in an HttpOnly session cookie.

## Layout

- `Caddyfile` — Basic Auth + reverse proxy to `127.0.0.1:8090`
- `eval-private.service` — static UI/API process
- `/etc/eval/private-site.env` — API token, signing key, Basic Auth env
- `/srv/private-ui` — generated static UI
- `/var/lib/eval-runs` — canonical run store

## Configure

```bash
sudo install -d -m 700 -o root -g root /etc/eval
read -rsp 'Basic Auth password: ' BASIC_PASSWORD; echo
BASIC_HASH=$(caddy hash-password --plaintext "$BASIC_PASSWORD")
sudo sh -c 'umask 077; \
  printf "EVAL_SITE_API_TOKEN=%s\n" "$(openssl rand -hex 32)" \
    > /etc/eval/private-site.env; \
  printf "EVAL_SITE_SIGNING_KEY=%s\n" "$(openssl rand -hex 32)" \
    >> /etc/eval/private-site.env; \
  printf "EVAL_BASIC_AUTH_USER=amaga\n" >> /etc/eval/private-site.env'
sudo sh -c "printf 'EVAL_BASIC_AUTH_HASH=%s\n' '$BASIC_HASH' \
  >> /etc/eval/private-site.env"
unset BASIC_PASSWORD BASIC_HASH
```

The API token is printed nowhere. Read it with `sudo cat` only when you
need to enter it into the login page.

## Run

```bash
sudo cp deploy/private/eval-private.service deploy/private/eval-caddy.service \
  /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now eval-private.service eval-caddy.service
```

## Cloudflare Tunnel

```bash
cloudflared tunnel --url http://127.0.0.1:8080
```

Do not open port 8080 publicly. The UI is private through the tunnel.

## Generate and publish

```bash
cp -R web/private_ui/. /srv/private-ui/
```

The static UI is served only after Basic Auth and the API session cookie.

