import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from transcribe_recording import normalize_result


class RecordingTranscriptionTests(unittest.TestCase):
    def test_timed_transcript_and_tokens_are_separate(self):
        result = normalize_result({"sentence_info": [{"start": 0, "end": 150, "text": "你好。"}], "words": ["你", "好"], "timestamp": [[0, 50], [50, 150]]})
        self.assertEqual(result["transcript"]["segments"][0]["text"], "你好。")
        self.assertEqual(len(result["timed_tokens"]["tokens"]), 2)
        self.assertIsNone(result["timing_warning"])

    def test_missing_tokens_do_not_invent_precision(self):
        result = normalize_result({"sentence_info": [{"start": 20, "end": 120, "text": "你好。"}]})
        self.assertIsNone(result["timed_tokens"])
        self.assertIsNotNone(result["timing_warning"])
        self.assertEqual(result["transcript"]["segments"][0]["start_ms"], 20)

    def test_missing_or_reversed_sentence_timing_is_rejected(self):
        for result in [{"text": "No timing"}, {"sentence_info": [{"start": 100, "end": 0, "text": "你好"}]}]:
            with self.assertRaises(ValueError):
                normalize_result(result)

    def test_bad_tokens_fall_back_to_coarse_timing(self):
        result = normalize_result({"sentence_info": [{"start": 0, "end": 150, "text": "你好。"}], "words": ["你", "好"], "timestamp": [[0, 100], [50, 150]]})
        self.assertIsNone(result["timed_tokens"])
        self.assertIn("inconsistent", result["timing_warning"])
