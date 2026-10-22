import io
import json
import os
import pathlib
import sys
import tempfile
import time
import types
import unittest
from contextlib import redirect_stdout
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from scripts import retention, verify_store


def write_json(path: pathlib.Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def write_text(path: pathlib.Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8")


def run_main(module: types.ModuleType, *args: str) -> tuple[int, str]:
    output = io.StringIO()
    with mock.patch("sys.argv", [module.__name__, *args]), redirect_stdout(output):
        code = module.main()
    return code, output.getvalue()


class VerifyStoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.store = self.root / "store"
        self.store.mkdir()

    def test_missing_store_is_reported(self) -> None:
        missing = self.root / "missing"
        code, output = run_main(verify_store, str(missing))
        self.assertEqual(code, 2)
        self.assertIn("store is not a directory", output)

    def test_empty_store_is_rejected(self) -> None:
        errors = verify_store.validate_store(self.store)
        self.assertEqual(errors, [f"no run directories found under {self.store}"])

    def test_invalid_metadata_is_rejected(self) -> None:
        run = self.store / "runs" / "suite" / "ref" / "run-bad-json"
        write_text(run / "metadata.json", "{not json")
        errors = verify_store.validate_store(self.store)
        self.assertEqual(len(errors), 1)
        self.assertIn("invalid metadata", errors[0])

    def test_non_object_metadata_is_rejected(self) -> None:
        run = self.store / "runs" / "suite" / "ref" / "run-array"
        write_json(run / "metadata.json", [])
        errors = verify_store.validate_store(self.store)
        self.assertEqual(len(errors), 1)
        self.assertIn("metadata is not an object", errors[0])

    def test_missing_metadata_and_invalid_manifest_are_rejected(self) -> None:
        missing = self.store / "runs" / "suite" / "ref" / "run-missing"
        missing.mkdir(parents=True)

        invalid = self.store / "runs" / "suite" / "ref" / "run-manifest"
        write_json(
            invalid / "metadata.json",
            {"cases": [{"case_id": "demo", "pass": True, "trials": []}]},
        )
        write_text(invalid / "manifest.json", "{not json")

        errors = verify_store.validate_store(self.store)
        self.assertEqual(len(errors), 2)
        self.assertTrue(any("missing metadata" in error for error in errors))
        self.assertTrue(any("invalid manifest" in error for error in errors))

    def test_valid_run_is_accepted(self) -> None:
        run = self.store / "runs" / "suite" / "ref" / "run-good"
        write_json(
            run / "metadata.json",
            {"run_id": "good", "cases": [{"case_id": "demo", "pass": True, "trials": []}]},
        )
        write_json(run / "manifest.json", {"manifest_version": 1, "lane": "replay"})

        self.assertEqual(verify_store.validate_store(self.store), [])
        code, output = run_main(verify_store, str(self.store))
        self.assertEqual(code, 0)
        self.assertIn("store OK: 1 run(s)", output)

    def test_legacy_run_without_manifest_is_accepted(self) -> None:
        run = self.store / "runs" / "suite" / "ref" / "run-legacy"
        write_json(
            run / "metadata.json",
            {"run_id": "legacy", "cases": [{"case_id": "demo", "pass": True, "trials": []}]},
        )

        self.assertEqual(verify_store.validate_store(self.store), [])
        code, _ = run_main(verify_store, str(self.store))
        self.assertEqual(code, 0)


class RetentionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = pathlib.Path(self.temp.name) / "store"
        self.store.mkdir()

    def make_run(self, suite: str, ref: str, name: str, age_days: float) -> pathlib.Path:
        run = self.store / "runs" / suite / ref / name
        write_text(run / "metadata.json", "{}")
        stamp = time.time() - age_days * 86400
        os.utime(run, (stamp, stamp))
        return run

    def run_retention(self, *args: str) -> tuple[int, str]:
        return run_main(retention, "--store", str(self.store), *args)

    def test_dry_run_keeps_all_runs_and_plans_only_older_ones(self) -> None:
        runs = [
            self.make_run("research-agents-service", "demo", f"run-{index:03d}", 400)
            for index in range(1, 5)
        ]

        code, output = self.run_retention("--keep-latest", "2", "--days", "30")

        self.assertEqual(code, 0)
        self.assertEqual(output.count("PLAN:"), 2)
        self.assertNotIn("DELETE:", output)
        self.assertIn("2 run(s) selected (dry-run)", output)
        self.assertTrue(all(run.is_dir() for run in runs))

    def test_apply_deletes_only_old_runs_outside_the_newest_n_per_group(self) -> None:
        keep_recent = self.make_run("research-agents-service", "demo", "run-001", 1)
        delete_old = self.make_run("research-agents-service", "demo", "run-002", 400)
        keep_old = self.make_run("research-agents-service", "demo", "run-003", 400)
        keep_newest = self.make_run("research-agents-service", "demo", "run-004", 1)

        digest_old = self.make_run("digest-agent", "baseline", "run-001", 400)
        digest_old_protected = self.make_run("digest-agent", "baseline", "run-002", 400)
        digest_newest = self.make_run("digest-agent", "baseline", "run-003", 1)

        code, output = self.run_retention("--keep-latest", "2", "--days", "30", "--apply")

        self.assertEqual(code, 0)
        self.assertEqual(output.count("DELETE:"), 2)
        self.assertIn("2 run(s) deleted", output)
        self.assertTrue(keep_recent.is_dir())
        self.assertFalse(delete_old.exists())
        self.assertTrue(keep_old.is_dir())
        self.assertTrue(keep_newest.is_dir())
        self.assertFalse(digest_old.exists())
        self.assertTrue(digest_old_protected.is_dir())
        self.assertTrue(digest_newest.is_dir())

    def test_apply_groups_runs_by_suite_and_ref(self) -> None:
        demo_old = self.make_run("suite", "demo", "run-001", 400)
        demo_new = self.make_run("suite", "demo", "run-002", 1)
        work_old = self.make_run("suite", "work", "run-001", 400)
        work_new = self.make_run("suite", "work", "run-002", 1)

        code, output = self.run_retention("--keep-latest", "1", "--days", "30", "--apply")

        self.assertEqual(code, 0)
        self.assertEqual(output.count("DELETE:"), 2)
        self.assertFalse(demo_old.exists())
        self.assertTrue(demo_new.is_dir())
        self.assertFalse(work_old.exists())
        self.assertTrue(work_new.is_dir())

    def test_dry_run_does_not_delete_anything(self) -> None:
        old = self.make_run("suite", "ref", "run-001", 400)
        newest = self.make_run("suite", "ref", "run-002", 1)

        code, output = self.run_retention("--keep-latest", "1", "--days", "30")

        self.assertEqual(code, 0)
        self.assertIn("PLAN:", output)
        self.assertTrue(old.is_dir())
        self.assertTrue(newest.is_dir())

    def test_invalid_retention_arguments_are_rejected(self) -> None:
        with self.assertRaises(SystemExit):
            self.run_retention("--keep-latest", "0", "--days", "30")

    def test_missing_store_is_rejected(self) -> None:
        self.store.rmdir()
        with self.assertRaises(SystemExit):
            self.run_retention("--keep-latest", "1", "--days", "30")


if __name__ == "__main__":
    unittest.main()
