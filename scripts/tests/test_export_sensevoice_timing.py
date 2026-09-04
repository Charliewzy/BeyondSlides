from __future__ import annotations

import sys
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import export_sensevoice_timing as exporter


class SenseVoiceTimingExportTests(unittest.TestCase):
    def test_words_and_timestamps_become_timed_tokens(self) -> None:
        timing = exporter.build_timed_transcript(
            {
                "words": ["说", "Rust"],
                "timestamp": [[150, 210], [330, 690]],
            }
        )

        self.assertEqual(
            timing,
            {
                "tokens": [
                    {"text": "说", "start_ms": 150, "end_ms": 210},
                    {"text": "Rust", "start_ms": 330, "end_ms": 690},
                ]
            },
        )

    def test_mismatched_word_and_timestamp_counts_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "2 words but 1 timestamps"):
            exporter.build_timed_transcript(
                {"words": ["不", "同"], "timestamp": [[0, 60]]}
            )


if __name__ == "__main__":
    unittest.main()
