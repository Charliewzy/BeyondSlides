import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from bundle_example_report import bundle


class BundleExampleTests(unittest.TestCase):
    def test_images_are_embedded_and_example_is_labeled(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "slide.png").write_bytes(b"example image")
            source = root / "report.html"
            source.write_text('<main><img src="slide.png"><span>讲稿</span></main>', encoding="utf-8")
            report = bundle(source)
            self.assertIn("data:image/png;base64,ZXhhbXBsZSBpbWFnZQ==", report)
            self.assertIn("data-example-notice", report)
            self.assertIn("讲稿", report)
            self.assertNotIn('src="slide.png"', report)

    def test_audio_and_images_outside_report_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "report.html"
            for content in ['<audio src="recording.flac"></audio>', '<img src="../outside.png">']:
                source.write_text(content, encoding="utf-8")
                with self.assertRaises(ValueError):
                    bundle(source)


if __name__ == "__main__":
    unittest.main()
