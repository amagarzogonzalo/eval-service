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


if __name__ == "__main__":
    unittest.main()
