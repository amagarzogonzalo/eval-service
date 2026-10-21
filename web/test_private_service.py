import pathlib
import sys
import time
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import private_service


class SessionTests(unittest.TestCase):
    def test_round_trip(self) -> None:
        key = b"test-signing-key"
        cookie = private_service.sign_session(key, int(time.time()) + 60)
        self.assertTrue(private_service.valid_session(key, cookie))

    def test_tampered_value(self) -> None:
        key = b"test-signing-key"
        cookie = private_service.sign_session(key, int(time.time()) + 60)
        self.assertFalse(private_service.valid_session(key, cookie + "x"))

    def test_expired_value(self) -> None:
        key = b"test-signing-key"
        cookie = private_service.sign_session(key, int(time.time()) - 1)
        self.assertFalse(private_service.valid_session(key, cookie))

    def test_wrong_key(self) -> None:
        cookie = private_service.sign_session(b"key-a", int(time.time()) + 60)
        self.assertFalse(private_service.valid_session(b"key-b", cookie))


class PathTests(unittest.TestCase):
    def test_safe_components(self) -> None:
        self.assertTrue(private_service.safe_component("run-123"))
        self.assertTrue(private_service.safe_component("digest-agent-e2e"))

    def test_unsafe_components(self) -> None:
        for value in ("", ".", "..", "../secret", "a/b", "a\\b"):
            self.assertFalse(private_service.safe_component(value))


class ModelTests(unittest.TestCase):
    def test_models_are_deduplicated(self) -> None:
        meta = {
            "cases": [
                {"trials": [{"output": {"output": {"data": {"usage": {"models": ["free-a"]}}}}}]},
                {"trials": [{"output": {"output": {"data": {"usage": {"models": ["free-a", "free-b"]}}}}}]},
            ]
        }
        self.assertEqual(private_service.find_models(meta), ["free-a", "free-b"])

    def test_missing_models_are_empty(self) -> None:
        self.assertEqual(private_service.find_models({"cases": [{"trials": []}]}), [])

    def test_content_picks_largest_artifact(self) -> None:
        meta = {
            "cases": [
                {"case_id": "health", "trials": [{"output": {"output": {"data": {"nsources": 1}}}}]},
                {
                    "case_id": "research",
                    "trials": [
                        {
                            "output": {
                                "output": {
                                    "data": {
                                        "nsources": 39,
                                        "citations": 14,
                                        "artifact_chars": 14978,
                                        "status": "pass",
                                    }
                                }
                            }
                        }
                    ],
                },
            ]
        }
        content = private_service.find_content(meta)
        self.assertEqual(content["nsources"], 39)
        self.assertEqual(content["citations"], 14)
        self.assertEqual(content["status"], "pass")


if __name__ == "__main__":
    unittest.main()

