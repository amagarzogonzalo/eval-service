import pathlib
import unittest

HERE = pathlib.Path(__file__).parent
APP = (HERE / "private_ui" / "app.js").read_text(encoding="utf-8")
STYLES = (HERE / "private_ui" / "styles.css").read_text(encoding="utf-8")


class PrivateUiContractTests(unittest.TestCase):
    def test_benchmark_contract_is_present(self) -> None:
        for marker in (
            'id="bench-query"',
            'id="bench-model"',
            'id="bench-legacy"',
            "Disaggregated by target + LLM",
            "bench-model-cards",
        ):
            self.assertIn(marker, APP)

    def test_evidence_and_judge_views_are_present(self) -> None:
        for marker in (
            "function renderMarkdown",
            "function renderJudge",
            "function cleanSnippet",
            "source-card",
            "score-row",
        ):
            self.assertIn(marker, APP)

    def test_responsive_style_support_is_present(self) -> None:
        self.assertIn(".judge-grid", STYLES)
        self.assertIn(".source-grid", STYLES)
        self.assertIn("@media", STYLES)


if __name__ == "__main__":
    unittest.main()
