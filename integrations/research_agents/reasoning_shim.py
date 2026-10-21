"""Loopback shim that disables provider reasoning before eval-proxy.

Free OpenRouter models expose a reasoning mode that can consume the whole
``max_tokens`` budget and return an empty ``content``. The target's synthesis
node cannot set OpenRouter's ``reasoning`` field, so this shim adds
``"reasoning": {"enabled": false}`` to each JSON request and forwards it to the
eval-proxy, which still owns the key and records usage. The shim never sees or
stores a provider key.
"""

from __future__ import annotations

import argparse
import http.client
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


class Handler(BaseHTTPRequestHandler):
    upstream_host = "127.0.0.1"
    upstream_port = 8797
    upstream_prefix = "/v1"

    def log_message(self, fmt: str, *args: object) -> None:
        return

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length)
        try:
            payload = json.loads(raw) if raw else None
        except ValueError:
            payload = None
        if isinstance(payload, dict) and "reasoning" not in payload:
            payload["reasoning"] = {"enabled": False}
            raw = json.dumps(payload, ensure_ascii=False).encode("utf-8")

        prefix = self.upstream_prefix.rstrip("/")
        path = self.path if self.path.startswith(prefix + "/") else prefix + self.path
        connection = http.client.HTTPConnection(
            self.upstream_host, self.upstream_port, timeout=180
        )
        headers = {
            key: value
            for key, value in self.headers.items()
            if key.lower() not in {"host", "content-length", "connection"}
        }
        headers["Content-Length"] = str(len(raw))
        try:
            connection.request("POST", path, body=raw, headers=headers)
            response = connection.getresponse()
            body = response.read()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() in {"transfer-encoding", "connection", "content-length"}:
                    continue
                self.send_header(key, value)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except OSError as exc:
            message = json.dumps({"error": {"message": f"shim upstream failed: {exc}"}}).encode()
            self.send_response(502)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)
        finally:
            connection.close()

    def do_GET(self) -> None:
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--upstream", required=True)
    args = parser.parse_args(argv)
    parts = urlsplit(args.upstream)
    Handler.upstream_host = parts.hostname or "127.0.0.1"
    Handler.upstream_port = parts.port or 8797
    Handler.upstream_prefix = parts.path.rstrip("/") or "/v1"
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    sys.stderr.write(f"reasoning shim on 127.0.0.1:{args.port} -> {args.upstream}\n")
    sys.stderr.flush()
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
