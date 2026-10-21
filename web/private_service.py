"""Private read-only web service for eval-service evidence.

Runs behind Caddy. Caddy enforces Basic Auth; this service exchanges a
separate API token for an HttpOnly session cookie and serves the static
UI plus read-only run metadata. No write endpoints.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import hmac
import json
import mimetypes
import os
import pathlib
import time
import urllib.parse
from http import HTTPStatus
from http.cookies import CookieError, SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SESSION_COOKIE = "eval_session"
SESSION_TTL_S = 12 * 60 * 60
MAX_POST_BYTES = 4096

LOGIN_PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>eval-service login</title>
<style>
body{font:15px/1.5 system-ui,sans-serif;background:#0b1020;color:#e8edff;
display:grid;place-items:center;min-height:100vh;margin:0}
main{background:#141b30;border:1px solid #26314f;border-radius:16px;padding:28px;
width:min(420px,90vw);box-shadow:0 20px 50px #0006}
h1{font-size:22px;margin:0 0 8px}p{color:#93a0c7;margin:0 0 20px}
label{display:block;font-size:12px;color:#93a0c7;margin-bottom:6px}
input{width:100%;box-sizing:border-box;padding:10px;border-radius:8px;
border:1px solid #26314f;background:#0b1020;color:#e8edff}
button{margin-top:14px;width:100%;padding:10px;border:0;border-radius:8px;
background:#7aa2ff;color:#0b1020;font-weight:700;cursor:pointer}
.error{color:#ff6b7a;font-size:13px;margin-top:10px}
</style>
</head>
<body>
<main>
<h1>eval-service</h1>
<p>Enter the API token for this private benchmark service.</p>
<form method="post" action="/session">
<label for="token">API token</label>
<input id="token" name="token" type="password" autocomplete="current-password" required>
<button type="submit">Unlock</button>
</form>
%s
</main>
</body>
</html>
"""


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).decode("ascii").rstrip("=")


def unb64(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def sign_session(key: bytes, expiry: int) -> str:
    payload = str(expiry).encode("ascii")
    mac = hmac.new(key, payload, hashlib.sha256).digest()
    return f"{expiry}.{b64(mac)}"


def valid_session(key: bytes, value: str) -> bool:
    try:
        expiry_text, mac_text = value.split(".", 1)
        expiry = int(expiry_text)
    except (ValueError, TypeError):
        return False
    if expiry < int(time.time()):
        return False
    expected = hmac.new(key, expiry_text.encode("ascii"), hashlib.sha256).digest()
    try:
        supplied = unb64(mac_text)
    except (binascii.Error, ValueError, TypeError):
        return False
    return hmac.compare_digest(expected, supplied)


def json_bytes(value: object) -> bytes:
    return json.dumps(value, indent=2, sort_keys=True).encode("utf-8")


def load_json(path: pathlib.Path) -> object | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None


def safe_component(value: str) -> bool:
    return bool(value) and value not in (".", "..") and "/" not in value and "\\" not in value


def run_summary(run_dir: pathlib.Path) -> dict | None:
    meta = load_json(run_dir / "metadata.json")
    if not isinstance(meta, dict):
        return None
    manifest = load_json(run_dir / "manifest.json")
    trace = load_json(run_dir / "trace.json")
    cases = meta.get("cases") or []
    return {
        "run_id": meta.get("run_id", run_dir.name.removeprefix("run-")),
        "suite": meta.get("suite"),
        "ref": meta.get("ref_name"),
        "code_version": meta.get("code_version"),
        "started_at_ms": meta.get("started_at_ms"),
        "trials_total": meta.get("trials_total", 0),
        "cases_total": len(cases),
        "cases_passed": sum(1 for case in cases if case.get("pass")),
        "pass": bool(cases) and all(case.get("pass") for case in cases),
        "manifest": manifest,
        "trace": trace,
        "path": str(run_dir),
    }


class Handler(BaseHTTPRequestHandler):
    server_version = "eval-private/0.1"

    def log_message(self, fmt: str, *args: object) -> None:
        print(f"{self.address_string()} - {fmt % args}", flush=True)

    @property
    def config(self) -> argparse.Namespace:
        return self.server.config  # type: ignore[attr-defined]

    def send_common_headers(self) -> None:
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("X-Frame-Options", "DENY")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header(
            "Content-Security-Policy",
            "default-src 'self'; style-src 'self' 'unsafe-inline'; "
            "script-src 'self'; img-src 'self' data:; connect-src 'self'; "
            "frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        )
        self.send_header("Cache-Control", "no-store")

    def cookie_session(self) -> str:
        raw = self.headers.get("Cookie", "")
        cookie = SimpleCookie()
        try:
            cookie.load(raw)
        except (CookieError, ValueError):
            return ""
        morsel = cookie.get(SESSION_COOKIE)
        return morsel.value if morsel else ""

    def session_ok(self) -> bool:
        return valid_session(self.config.signing_key, self.cookie_session())

    def respond(self, status: int, body: bytes, content_type: str) -> None:
        self.send_response(status)
        self.send_common_headers()
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def respond_json(self, status: int, value: object) -> None:
        self.respond(status, json_bytes(value), "application/json; charset=utf-8")

    def redirect(self, location: str) -> None:
        self.send_response(HTTPStatus.SEE_OTHER)
        self.send_common_headers()
        self.send_header("Location", location)
        self.end_headers()

    def login_page(self, error: bool = False) -> None:
        note = '<p class="error">Invalid token.</p>' if error else ""
        self.respond(
            HTTPStatus.OK,
            LOGIN_PAGE.replace("%s", note).encode("utf-8"),
            "text/html; charset=utf-8",
        )

    def require_session(self, api: bool) -> bool:
        if self.session_ok():
            return True
        if api:
            self.respond_json(HTTPStatus.UNAUTHORIZED, {"error": "unauthorized"})
        else:
            self.redirect("/login")
        return False

    def do_GET(self) -> None:
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        if path == "/healthz":
            self.respond_json(HTTPStatus.OK, {"ok": True})
            return
        if path == "/login":
            if self.session_ok():
                self.redirect("/")
            else:
                self.login_page()
            return
        if path == "/logout":
            self.send_response(HTTPStatus.SEE_OTHER)
            self.send_common_headers()
            self.send_header(
                "Set-Cookie",
                f"{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict",
            )
            self.send_header("Location", "/login")
            self.end_headers()
            return
        if path.startswith("/api/"):
            if not self.require_session(api=True):
                return
            self.api_get(path, urllib.parse.parse_qs(parsed.query))
            return
        if not self.require_session(api=False):
            return
        self.static_get(path)

    def do_POST(self) -> None:
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != "/session":
            self.respond_json(HTTPStatus.NOT_FOUND, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length <= 0 or length > MAX_POST_BYTES:
            self.login_page(error=True)
            return
        body = self.rfile.read(length).decode("utf-8", "replace")
        form = urllib.parse.parse_qs(body)
        token = (form.get("token") or [""])[0]
        if not hmac.compare_digest(token, self.config.api_token):
            time.sleep(0.25)
            self.login_page(error=True)
            return
        expiry = int(time.time()) + SESSION_TTL_S
        value = sign_session(self.config.signing_key, expiry)
        secure = "" if self.config.allow_http_cookie else "; Secure"
        cookie = (
            f"{SESSION_COOKIE}={value}; Path=/; Max-Age={SESSION_TTL_S}; "
            f"HttpOnly{secure}; SameSite=Strict"
        )
        self.send_response(HTTPStatus.SEE_OTHER)
        self.send_common_headers()
        self.send_header("Set-Cookie", cookie)
        self.send_header("Location", "/")
        self.end_headers()

    def api_get(self, path: str, query: dict[str, list[str]]) -> None:
        store = self.config.store
        if path == "/api/runs":
            runs = []
            for run_dir in sorted(store.glob("runs/*/*/run-*"), reverse=True):
                summary = run_summary(run_dir)
                if summary:
                    runs.append(summary)
            self.respond_json(HTTPStatus.OK, {"runs": runs})
            return
        if path == "/api/run":
            suite = (query.get("suite") or [""])[0]
            ref = (query.get("ref") or [""])[0]
            run_id = (query.get("run_id") or [""])[0]
            if not all(safe_component(value) for value in (suite, ref, run_id)):
                self.respond_json(HTTPStatus.BAD_REQUEST, {"error": "bad run identity"})
                return
            run_dir = store / "runs" / suite / ref / ("run-" + run_id)
            if not run_dir.is_dir():
                self.respond_json(HTTPStatus.NOT_FOUND, {"error": "run not found"})
                return
            files = {}
            for name in (
                "metadata.json",
                "manifest.json",
                "grades.json",
                "decision.json",
                "trace.json",
                "output-metrics.json",
                "observations.jsonl",
                "transcript.jsonl",
                "stdout.log",
                "stderr.log",
            ):
                path_file = run_dir / name
                if path_file.is_file():
                    files[name] = path_file.read_text(encoding="utf-8", errors="replace")
            self.respond_json(
                HTTPStatus.OK,
                {"run": run_summary(run_dir), "files": files, "query": query},
            )
            return
        if path == "/api/health":
            self.respond_json(HTTPStatus.OK, {"ok": True, "service": "eval-private"})
            return
        self.respond_json(HTTPStatus.NOT_FOUND, {"error": "not found"})

    def static_get(self, path: str) -> None:
        root = self.config.root
        relative = path.lstrip("/") or "index.html"
        candidate = (root / relative).resolve()
        if root not in candidate.parents and candidate != root:
            self.respond_json(HTTPStatus.FORBIDDEN, {"error": "forbidden"})
            return
        if candidate.is_dir():
            candidate = candidate / "index.html"
        if not candidate.is_file():
            self.respond_json(HTTPStatus.NOT_FOUND, {"error": "not found"})
            return
        content_type = mimetypes.guess_type(candidate.name)[0] or "application/octet-stream"
        if content_type.startswith("text/") or candidate.suffix == ".html":
            content_type += "; charset=utf-8"
        self.respond(HTTPStatus.OK, candidate.read_bytes(), content_type)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--store", type=pathlib.Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8090)
    parser.add_argument("--allow-http-cookie", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    args.root = args.root.resolve()
    args.store = args.store.resolve()
    token = os.environ.get("EVAL_SITE_API_TOKEN", "")
    key_text = os.environ.get("EVAL_SITE_SIGNING_KEY", "")
    if not token or not key_text:
        raise SystemExit("EVAL_SITE_API_TOKEN and EVAL_SITE_SIGNING_KEY are required")
    args.api_token = token
    args.signing_key = key_text.encode("utf-8")
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.config = args  # type: ignore[attr-defined]
    print(f"serving {args.root} on http://{args.host}:{args.port}", flush=True)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())




