import sys
import unittest
from pathlib import Path
import tempfile
import json

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from transcribe_recording import normalize_result, TranscriptionProgress, observed_model_type


class RecordingTranscriptionTests(unittest.TestCase):
    def test_progress_observes_calls_without_modifying_inputs_or_results(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "progress.json"
            now = [0.0]
            progress = TranscriptionProgress(path, 42, 10000, clock=lambda: now[0])
            class Base:
                vad_model, punc_model, model = object(), object(), object()
                def inference(self, value, **kwargs):
                    self.received = value
                    return self.result
            model = observed_model_type(Base, progress)()
            now[0] = 2
            model.result = [{"value": [[0, 1000], [2000, 5000]]}]
            self.assertIs(model.inference("audio", model=model.vad_model), model.result)
            self.assertEqual(progress.state["total_speech_ms"], 4000)
            now[0] = 3
            audio = [range(16000)]
            model.result = [{"text": "unchanged", "timestamp": [[0, 1000]]}]
            self.assertIs(model.inference(audio, model=model.model), model.result)
            self.assertIs(model.received, audio)
            snapshot = json.loads(path.read_text())
            self.assertEqual(snapshot["completed_speech_ms"], 1000)
            self.assertEqual(snapshot["completed_regions"], 1)
            self.assertEqual(snapshot["total_regions"], 2)
            self.assertEqual(snapshot["attempt_started_ms"], 42)
            now[0] = 4
            model.inference([range(48000)], model=model.model)
            self.assertEqual(json.loads(path.read_text())["completed_speech_ms"], 4000)
            progress.phase("punctuating")
            self.assertEqual(progress.state["timings_seconds"]["recognizing"], 2)

    def test_unrecognized_library_shapes_leave_progress_indeterminate(self):
        with tempfile.TemporaryDirectory() as directory:
            progress = TranscriptionProgress(Path(directory) / "progress.json", 0, 10000)
            progress.detected([{"unexpected": []}])
            progress.recognized("not an audio batch", [])
            self.assertIsNone(progress.state["total_regions"])
            self.assertIsNone(progress.state["total_speech_ms"])

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
