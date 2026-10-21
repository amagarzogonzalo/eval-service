"""Offline tests for the reasoning-disable shim."""

from __future__ import annotations

import json
import threading
import unittest
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import ClassVar

import reasoning_shim


class Upstream(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    payload: ClassVar[dict] = {}
    path: ClassVar[str] = ""

    def log_message(self, fmt: str, *args: object) -> None:
        return

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        Upstream.payload = json.loads(self.rfile.read(length))
        Upstream.path = self.path
        body = json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class ReasoningShimTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.upstream = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        threading.Thread(target=cls.upstream.serve_forever, daemon=True).start()
        reasoning_shim.Handler.upstream_host = "127.0.0.1"
        reasoning_shim.Handler.upstream_port = cls.upstream.server_port
        reasoning_shim.Handler.upstream_prefix = "/v1"
        cls.shim = ThreadingHTTPServer(("127.0.0.1", 0), reasoning_shim.Handler)
        threading.Thread(target=cls.shim.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.shim.shutdown()
        cls.upstream.shutdown()

    def post(self, path: str) -> dict:
        body = json.dumps(
            {"model": "free-model", "messages": [{"role": "user", "content": "hi"}]}
        ).encode()
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.shim.server_port}{path}",
            data=body,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=10) as response:
            return json.load(response)

    def test_adds_reasoning_disabled_and_forwards_v1_path(self) -> None:
        result = self.post("/v1/chat/completions")
        self.assertEqual(result["choices"][0]["message"]["content"], "ok")
        self.assertEqual(Upstream.path, "/v1/chat/completions")
        self.assertEqual(Upstream.payload["reasoning"], {"enabled": False})

    def test_adds_prefix_for_bare_path(self) -> None:
        self.post("/chat/completions")
        self.assertEqual(Upstream.path, "/v1/chat/completions")

    def test_keeps_explicit_reasoning(self) -> None:
        body = json.dumps(
            {
                "model": "free-model",
                "messages": [],
                "reasoning": {"enabled": True},
            }
        ).encode()
        req = urllib.request.Request(
            f"http://127.0.0.1:{self.shim.server_port}/v1/chat/completions",
            data=body,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=10):
            pass
        self.assertEqual(Upstream.payload["reasoning"], {"enabled": True})


if __name__ == "__main__":
    unittest.main()
