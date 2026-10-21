"""Offline tests for the black-box research service client.

Runs against a stdlib HTTP server; no network, no model calls, no secrets.
"""

from __future__ import annotations

import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import ClassVar

import service_runner

TOKEN = "test-token-not-a-secret"


def artifact_text() -> str:
    body = " ".join(["evidence"] * 250)
    return (
        "---\n"
        "'@id': eval-service\n"
        "type: article\n"
        "title: test\n"
        "provenance: sources\n"
        "citations: [a, b, c]\n"
        "tldr_llm: short\n"
        "---\n\n"
        "## One\n"
        f"{body} [^a] [^b] [^c]\n\n"
        "## Two\n"
        f"{body}\n"
    )


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"
    state: ClassVar[dict] = {}

    def log_message(self, fmt: str, *args: object) -> None:
        return

    def _send(self, status: int, body: object, content_type: str = "application/json") -> None:
        if isinstance(body, (dict, list)):
            raw = json.dumps(body).encode()
        elif isinstance(body, bytes):
            raw = body
        else:
            raw = str(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(raw)))
        if status == 429:
            self.send_header("Retry-After", "1")
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self) -> None:
        state = self.state
        if self.path == "/health":
            if state.get("health_malformed"):
                self._send(200, b"{not-json", "application/json")
                return
            self._send(200, {"status": "ok", "engine": "ready"})
            return
        if self.path == "/readiness":
            self._send(200, {"ok": True, "checks": {"providers": True, "auth": True}})
            return
        if self.path == "/result/task-1":
            polls = state.setdefault("polls", [])
            if polls:
                status = polls.pop(0)
            else:
                status = "done"
            if state.get("malformed_poll"):
                self._send(200, b"{not-json", "application/json")
                return
            self._send(200, {"task_id": "task-1", "thread_id": "thread-1", "status": status})
            return
        if self.path == "/result/task-1/artifact":
            if state.get("artifact_status", 200) != 200:
                self._send(state["artifact_status"], {"detail": "not done"})
                return
            self._send(200, artifact_text(), "text/markdown")
            return
        if self.path == "/result/task-1/sources":
            self._send(200, {"task_id": "task-1", "thread_id": "thread-1", "sources": [{}, {}, {}]})
            return
        self._send(404, {"detail": "not found"})

    def do_POST(self) -> None:
        state = self.state
        if self.path != "/research":
            self._send(404, {"detail": "not found"})
            return
        auth = self.headers.get("Authorization")
        if state.get("auth_fail") or auth != f"Bearer {TOKEN}":
            self._send(401, {"detail": "missing or invalid bearer token"})
            return
        state["submit_calls"] = state.get("submit_calls", 0) + 1
        if state.get("submit_429") and state["submit_calls"] == 1:
            self._send(429, {"detail": "queue full"})
            return
        self._send(202, {"task_id": "task-1", "status": "queued"})


class ServiceRunnerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.base_url = f"http://127.0.0.1:{cls.server.server_port}"
        cls.dataset = Path(tempfile.mkdtemp(prefix="service-test-")) / "topics.json"
        cls.dataset.write_text(
            json.dumps(
                {
                    "topics": [
                        {
                            "id": "t",
                            "kind": "research",
                            "query": "q",
                            "expected": {
                                "min_sources": 3,
                                "min_citations": 3,
                                "min_artifact_chars": 1500,
                            },
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.shutdown()
        cls.thread.join(timeout=5)

    def setUp(self) -> None:
        Handler.state = {}
        os.environ["EVAL_RESEARCH_SERVICE_TOKEN"] = TOKEN

    def tearDown(self) -> None:
        os.environ.pop("EVAL_RESEARCH_SERVICE_TOKEN", None)

    def run_client(self, **overrides: object) -> dict:
        out_dir = tempfile.mkdtemp(prefix="service-artifacts-")
        argv = [
            "--base-url",
            self.base_url,
            "--lane",
            "local",
            "--dataset",
            str(self.dataset),
            "--topic",
            "t",
            "--timeout-s",
            "3",
            "--poll-s",
            "0.1",
            "--poll-max-s",
            "0.2",
            "--artifact-out",
            out_dir,
        ]
        for key, value in overrides.items():
            flag = f"--{key.replace('_', '-')}"
            if isinstance(value, bool):
                if value:
                    argv.append(flag)
                continue
            argv.extend([flag, str(value)])
        result = service_runner.run(service_runner.parse_args(argv))
        if result.get("code") == "service_unreachable" and "10053" in " ".join(
            result.get("failures") or []
        ):
            # Windows loopback occasionally aborts a request after the server
            # closes; retry once so the test exercises the client logic.
            result = service_runner.run(service_runner.parse_args(argv))
        return result

    def test_token_falls_back_to_render_api_key(self) -> None:
        os.environ.pop("EVAL_RESEARCH_SERVICE_TOKEN", None)
        os.environ["RENDER_API_KEY"] = "render-app-token"
        try:
            self.assertEqual(service_runner.service_token(), "render-app-token")
        finally:
            os.environ.pop("RENDER_API_KEY", None)

    def test_health_only_needs_no_token(self) -> None:
        os.environ.pop("EVAL_RESEARCH_SERVICE_TOKEN", None)
        result = self.run_client(health_only=True)
        self.assertEqual(result["status"], "ready")
        self.assertTrue(result["service"]["effective_ready"])

    def test_healthy_run(self) -> None:
        Handler.state["polls"] = ["running"]
        result = self.run_client()
        self.assertEqual(result["status"], "pass", result["failures"])
        self.assertEqual(result["task_id"], "task-1")
        self.assertEqual(result["nsources"], 3)
        self.assertEqual(result["citations"], 3)
        self.assertEqual(result["service"]["poll_count"], 2)
        self.assertTrue(Path(result["artifact_path"]).is_file())
        self.assertFalse(result["model_policy"]["required"])

    def test_submit_429_then_success(self) -> None:
        Handler.state["submit_429"] = True
        result = self.run_client()
        self.assertEqual(result["status"], "pass", result["failures"])
        self.assertEqual(result["service"]["submit"]["attempts"], 2)

    def test_artifact_not_ready(self) -> None:
        Handler.state["artifact_status"] = 409
        result = self.run_client()
        self.assertEqual(result["status"], "fail")
        self.assertEqual(result["code"], "service_not_done")

    def test_timeout(self) -> None:
        Handler.state["polls"] = ["running"] * 100
        result = self.run_client(timeout_s="2", poll_s="0.1", poll_max_s="0.2")
        self.assertEqual(result["status"], "fail")
        self.assertEqual(result["code"], "service_timeout")

    def test_auth_failure(self) -> None:
        Handler.state["auth_fail"] = True
        result = self.run_client()
        self.assertEqual(result["status"], "fail")
        self.assertEqual(result["code"], "service_auth")

    def test_malformed_poll(self) -> None:
        Handler.state["malformed_poll"] = True
        result = self.run_client()
        self.assertEqual(result["status"], "fail")
        self.assertEqual(result["code"], "service_protocol")

    def test_token_never_serialized(self) -> None:
        Handler.state["polls"] = ["running"]
        result = self.run_client()
        self.assertNotIn(TOKEN, json.dumps(result))

    def _usage_store(self, model: str) -> str:
        store = tempfile.mkdtemp(prefix="service-usage-")
        directory = Path(store) / "usage" / "research-agents"
        directory.mkdir(parents=True)
        event = {
            "event": "llm.call",
            "event_version": 1,
            "event_id": "x" * 32,
            "project": "research-agents",
            "request_id": "req",
            "attempt": 0,
            "provider": "openrouter",
            "model": model,
            "started_at_ms": service_runner.now_ms(),
            "duration_ms": 1,
            "outcome": "ok",
            "tag": "test-tag",
        }
        (directory / "events-19700101.jsonl").write_text(json.dumps(event) + "\n", encoding="utf-8")
        return store

    def test_free_model_policy_accepts_free(self) -> None:
        Handler.state["polls"] = ["running"]
        store = self._usage_store("free-model")
        result = self.run_client(
            usage_store=store,
            usage_tag="test-tag",
            require_free_models=True,
            free_model_allowlist="free-model",
        )
        self.assertEqual(result["status"], "pass", result["failures"])
        self.assertTrue(result["model_policy"]["verified"])

    def test_free_model_policy_rejects_paid(self) -> None:
        Handler.state["polls"] = ["running"]
        store = self._usage_store("paid-model")
        result = self.run_client(
            usage_store=store,
            usage_tag="test-tag",
            require_free_models=True,
            free_model_allowlist="free-model",
        )
        self.assertEqual(result["status"], "fail")
        self.assertIn("non-free model served", " ".join(result["failures"]))


if __name__ == "__main__":
    unittest.main()
