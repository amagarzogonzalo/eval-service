import json
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

import export_calibration


class CalibrationExportTests(unittest.TestCase):
    def test_export_keeps_human_labels_empty(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            run = pathlib.Path(temp) / "runs" / "suite" / "ref" / "run-abc"
            run.mkdir(parents=True)
            (run / "artifact.md").write_text("Answer with [^1].", encoding="utf-8")
            metadata = {
                "run_id": "abc",
                "suite": "suite",
                "ref_name": "ref",
                "cases": [
                    {
                        "case_id": "research",
                        "trials": [
                            {
                                "output": {
                                    "output": {
                                        "text": "Answer",
                                        "data": {"query": "Question?", "status": "pass"},
                                    }
                                }
                            }
                        ],
                    }
                ],
            }
            (run / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
            (run / "manifest.json").write_text(
                json.dumps({"manifest_version": 1, "model": {"model": "free"}}),
                encoding="utf-8",
            )
            (run / "judge.json").write_text(
                json.dumps({"model": "judge", "overall": 9, "scores": {"overall": 9}}),
                encoding="utf-8",
            )
            rows = export_calibration.export_rows(pathlib.Path(temp), limit=5, max_chars=1000)
            self.assertEqual(len(rows), 1)
            self.assertIsNone(rows[0]["human_label"])
            self.assertIsNone(rows[0]["human_score"])
            self.assertEqual(rows[0]["judge_label"], "pass")
            self.assertEqual(rows[0]["result"], "Answer with [^1].")


if __name__ == "__main__":
    unittest.main()
