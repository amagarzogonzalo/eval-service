import http.client
import json
import pathlib
import sys
import tempfile
import threading
import unittest
import urllib.parse
from types import SimpleNamespace

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import private_service


def write_json(path: pathlib.Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


class PrivateApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        base = pathlib.Path(self.temp.name)
        self.store = base / "store"
        self.root = base / "ui"
        self.root.mkdir()
        (self.root / "index.html").write_text("demo", encoding="utf-8")
        artifact = self.store / "artifacts" / "answer.md"
        artifact.parent.mkdir(parents=True)
        artifact.write_text("# RAG answer\n\nGrounded answer.", encoding="utf-8")

        run = self.store / "runs" / "research-agents-service" / "demo" / "run-123"
        metadata = {
            "run_id": "123",
            "suite": "research-agents-service",
            "ref_name": "demo",
            "started_at_ms": 1,
            "trials_total": 1,
            "cases": [
                {
                    "case_id": "research-service-e2e",
                    "pass": True,
                    "trials": [
                        {
                            "output": {
                                "output": {
                                    "text": "answer",
                                    "data": {
                                        "query": "How does RAG reduce hallucinations?",
                                        "status": "pass",
                                        "nsources": 2,
                                        "citations": 3,
                                        "artifact_chars": 100,
                                        "artifact_path": str(artifact),
                                        "sources": [
                                            {"title": "Source A", "doi": "10.1/a"},
                                            {"title": "Source B", "doi": "10.1/b"},
                                        ],
                                    },
                                }
                            }
                        }
                    ],
                }
            ],
        }
        write_json(run / "metadata.json", metadata)
        write_json(
            run / "manifest.json",
            {
                "manifest_version": 1,
                "model": {"provider": "test", "model": "free"},
                "lane": "remote-service",
                "target": {"version": "demo"},
                "free_only": True,
            },
        )
        write_json(run / "grades.json", [])
        write_json(run / "judge.json", {"overall": 9, "scores": {"overall": 9}})

        self.server = private_service.ThreadingHTTPServer(
            ("127.0.0.1", 0), private_service.Handler
        )
        self.server.config = SimpleNamespace(
            root=self.root,
            store=self.store,
            api_token="token",
            signing_key=b"test-key",
            allow_http_cookie=True,
        )
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.port = self.server.server_address[1]
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.addCleanup(self.temp.cleanup)

    def request(
        self, method: str, path: str, body: str = "", cookie: str = ""
    ) -> tuple[int, bytes, str]:
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        headers = {}
        if body:
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        if cookie:
            headers["Cookie"] = cookie
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        payload = response.read()
        set_cookie = response.getheader("Set-Cookie") or ""
        connection.close()
        return response.status, payload, set_cookie

    def login(self) -> str:
        body = urllib.parse.urlencode({"token": "token"})
        status, _, set_cookie = self.request("POST", "/session", body)
        self.assertEqual(status, 303)
        return set_cookie.split(";", 1)[0]

    def test_requires_auth(self) -> None:
        status, payload, _ = self.request("GET", "/api/runs")
        self.assertEqual(status, 401)
        self.assertIn(b"unauthorized", payload)

    def test_run_api_loads_artifact_and_metadata(self) -> None:
        cookie = self.login()
        status, payload, _ = self.request("GET", "/api/runs", cookie=cookie)
        self.assertEqual(status, 200)
        runs = json.loads(payload)["runs"]
        self.assertEqual(len(runs), 1)
        self.assertFalse(runs[0]["legacy"])
        self.assertEqual(runs[0]["model"]["model"], "free")
        self.assertEqual(runs[0]["content"]["nsources"], 2)

        query = urllib.parse.urlencode(
            {"suite": "research-agents-service", "ref": "demo", "run_id": "123"}
        )
        status, payload, _ = self.request("GET", "/api/run?" + query, cookie=cookie)
        self.assertEqual(status, 200)
        detail = json.loads(payload)
        self.assertIn("RAG answer", detail["files"]["artifact.md"])
        self.assertEqual(len(json.loads(detail["files"]["sources.json"])), 2)
        self.assertEqual(detail["run"]["content"]["citations"], 3)

    def test_legacy_run_is_marked(self) -> None:
        run = self.store / "runs" / "research-agents-service" / "demo" / "run-000"
        write_json(
            run / "metadata.json",
            {
                "run_id": "000",
                "suite": "research-agents-service",
                "ref_name": "demo",
                "cases": [{"case_id": "legacy", "pass": True, "trials": []}],
            },
        )
        cookie = self.login()
        _, payload, _ = self.request("GET", "/api/runs", cookie=cookie)
        runs = json.loads(payload)["runs"]
        legacy = next(run for run in runs if run["run_id"] == "000")
        self.assertTrue(legacy["legacy"])


if __name__ == "__main__":
    unittest.main()



