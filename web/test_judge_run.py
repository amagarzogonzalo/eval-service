import base64
import pathlib
import sys
import unittest
import zlib

sys.path.insert(0, str(pathlib.Path(__file__).parent))

import judge_run


class JudgeTextTests(unittest.TestCase):
    def test_clean_text_decodes_entities_and_controls(self) -> None:
        value = "Use &amp; evidence \u007f with \u0097 punctuation"
        self.assertEqual(judge_run.clean_text(value), "Use & evidence with - punctuation")

    def test_pdf_text_extracts_ascii85_flate_text(self) -> None:
        content = b"BT (Hello RAG) Tj ET"
        compressed = zlib.compress(content)
        encoded = base64.a85encode(compressed) + b"~>"
        data = b"%PDF-1.4\nstream\n" + encoded + b"endstream\n"
        self.assertIn("Hello RAG", judge_run.pdf_text(data))

    def test_free_model_check(self) -> None:
        self.assertTrue(judge_run.is_free_model("provider/model:free"))
        self.assertTrue(judge_run.is_free_model("openrouter/free"))
        self.assertFalse(judge_run.is_free_model("provider/model"))


if __name__ == "__main__":
    unittest.main()
