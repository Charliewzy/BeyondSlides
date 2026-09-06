import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from render_passage_boundary_review import build_review


class BoundaryReviewTests(unittest.TestCase):
    def run_fixture(self, directory, texts, source=None):
        directory.mkdir()
        source = "".join(texts) if source is None else source
        artifact = {"restored_transcript": {"spans": [{"kind": "text", "text": source}]},
                    "passages": [{"text": text, "importance": 4, "novelty": 3} for text in texts]}
        (directory / "analysis.json").write_text(json.dumps(artifact))
        (directory / "report.html").write_text("".join(
            '<span data-passage data-audio-start-ms="0" data-audio-end-ms="1000" data-time="00:00"></span>'
            for _ in texts))

    def test_identical_text_can_have_different_partitions_and_is_html_safe(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            text = '中文🦀</script><script>window.injected=true</script>。'
            self.run_fixture(root / "old", [text[:2], text[2:]])
            self.run_fixture(root / "new", [text])
            data = build_review(root / "old", root / "new", root / "review.html")
            self.assertEqual(data["characters"], len(text))
            self.assertEqual(data["stats"]["old"]["passages"], 2)
            self.assertEqual(data["stats"]["new"]["passages"], 1)
            self.assertNotIn(text, (root / "review.html").read_text())
            self.assertIn("\\u003c/script>", (root / "review.html").read_text())

    def test_source_changes_are_rejected_instead_of_compared_as_segmentation(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.run_fixture(root / "old", ["原文。"])
            self.run_fixture(root / "new", ["改写。"])
            with self.assertRaisesRegex(ValueError, "same authoritative text"):
                build_review(root / "old", root / "new", root / "review.html")

    def test_report_timing_must_match_passage_count(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.run_fixture(root / "old", ["原文。"])
            self.run_fixture(root / "new", ["原文。"])
            (root / "new" / "report.html").write_text("")
            with self.assertRaisesRegex(ValueError, "counts disagree"):
                build_review(root / "old", root / "new", root / "review.html")


if __name__ == "__main__":
    unittest.main()
