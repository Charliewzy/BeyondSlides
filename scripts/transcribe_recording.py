"""CPU-only local ASR adapter invoked by the application worker.

Input is an ffmpeg-normalized local WAV. Output is one atomic bundle; an
interrupted inference never masquerades as a complete transcript checkpoint.
"""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import wave


class TranscriptionProgress:
    """Observe existing inference calls; never select, reorder, or split audio."""

    def __init__(self, path, attempt_started_ms, audio_ms, extraction_seconds=0, clock=time.monotonic):
        self.path, self.clock, self.audio_ms = path, clock, audio_ms
        self.state = dict(observer_version=1, phase="loading_models", attempt_started_ms=attempt_started_ms,
                          completed_regions=0, total_regions=None, completed_speech_ms=0,
                          total_speech_ms=None, timings_seconds={"extracting_audio": extraction_seconds})
        self.phase_started = clock()
        self.last_write = float("-inf")
        self.publish(force=True)

    def phase(self, phase):
        now = self.clock()
        previous = self.state["phase"]
        if previous != phase:
            timings = self.state["timings_seconds"]
            timings[previous] = timings.get(previous, 0) + now - self.phase_started
            self.phase_started = now
            self.state["phase"] = phase
            print(f"Transcription: {phase}", flush=True)
            self.publish(force=True)

    def detected(self, results):
        # This adapter supports the single recording, 16 kHz, unmerged VAD
        # configuration used below. Unknown library output stays indeterminate.
        try:
            regions = results[0]["value"]
            if len(results) != 1 or any(not (0 <= a <= b) for a, b in regions):
                raise ValueError("Unexpected speech-region structure")
            total_ms = sum(max(0, min(b, self.audio_ms) - a) for a, b in regions)
            self.state.update(total_regions=len(regions), total_speech_ms=round(total_ms))
        except (KeyError, IndexError, TypeError, ValueError):
            self.state.update(total_regions=None, total_speech_ms=None)
        self.phase("recognizing")

    def recognized(self, audio, results):
        try:
            if not isinstance(audio, list) or len(audio) != len(results):
                raise ValueError("Unexpected recognition batch")
            samples = sum(len(region) for region in audio)
            self.state["completed_regions"] += len(audio)
            self.state["completed_speech_ms"] += round(samples / 16)
            if (self.state["total_regions"] is not None and
                    self.state["completed_regions"] > self.state["total_regions"]):
                raise ValueError("Recognition no longer matches detected regions")
        except (TypeError, ValueError):
            self.state.update(total_regions=None, total_speech_ms=None)
        self.publish(force=self.state["completed_regions"] == self.state["total_regions"])

    def publish(self, force=False):
        now = self.clock()
        if force or now - self.last_write >= 0.2:
            write_json(self.path, self.state)
            self.last_write = now


def observed_model_type(base, progress):
    class ObservedModel(base):
        def inference(self, *args, **kwargs):
            model = kwargs.get("model")
            stage = ("detecting_speech" if model is self.vad_model else
                     "punctuating" if model is self.punc_model else "recognizing")
            progress.phase(stage)
            result = super().inference(*args, **kwargs)
            if stage == "detecting_speech":
                progress.detected(result)
            elif stage == "recognizing":
                progress.recognized(args[0] if args else kwargs.get("input"), result)
            return result
    return ObservedModel


def write_json(path, value):
    path = Path(path)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as output:
            temporary = output.name
            json.dump(value, output, ensure_ascii=False)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary and os.path.exists(temporary):
            os.unlink(temporary)


def normalize_result(result):
    sentences = result.get("sentence_info")
    if not isinstance(sentences, list) or not sentences:
        raise ValueError("ASR returned no timestamped transcript; refusing to invent timing")
    segments = []
    for sentence in sentences:
        text = str(sentence.get("text", sentence.get("sentence", ""))).strip()
        if not text:
            continue
        start, end = int(sentence["start"]), int(sentence["end"])
        if start < 0 or end < start:
            raise ValueError("ASR returned an invalid transcript interval")
        if segments and (start < segments[-1]["start_ms"] or end < segments[-1]["end_ms"]):
            raise ValueError("ASR returned out-of-order transcript intervals")
        segments.append(dict(id=len(segments), start_ms=start, end_ms=end, text=text))
    if not segments:
        raise ValueError("No speech was recognized in this recording")

    # Token timing is optional. If absent or inconsistent, preserve the coarse
    # transcript timestamps and explicitly report the unavailable precision.
    words, stamps = result.get("words"), result.get("timestamp")
    tokens = []
    token_warning = None
    try:
        if not isinstance(words, list) or not isinstance(stamps, list) or len(words) != len(stamps) or not words:
            raise ValueError("ASR did not return aligned words and timestamps")
        for word, stamp in zip(words, stamps):
            text = str(word).strip()
            start, end = int(stamp[0]), int(stamp[1])
            if not text or start < 0 or end <= start or (tokens and start < tokens[-1]["end_ms"]):
                raise ValueError("ASR token timing was inconsistent")
            tokens.append(dict(text=text, start_ms=start, end_ms=end))
    except (ValueError, TypeError, KeyError, IndexError) as error:
        tokens = []
        token_warning = str(error)
    return {"transcript": {"segments": segments}, "timed_tokens": {"tokens": tokens} if tokens else None, "timing_warning": token_warning}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("audio", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("progress", type=Path)
    parser.add_argument("--attempt-started-ms", type=int, default=0)
    parser.add_argument("--extraction-seconds", type=float, default=0)
    args = parser.parse_args()
    started = time.monotonic()
    with wave.open(str(args.audio), "rb") as audio:
        audio_ms = audio.getnframes() * 1000 / audio.getframerate()
    progress = TranscriptionProgress(args.progress, args.attempt_started_ms, audio_ms, args.extraction_seconds)
    from funasr import AutoModel
    from importlib.metadata import version
    model = observed_model_type(AutoModel, progress)(model="iic/SenseVoiceSmall", vad_model="fsmn-vad",
                      vad_kwargs={"max_single_segment_time": 30000}, punc_model="ct-punc",
                      device="cpu", disable_update=True, disable_pbar=True)
    progress.phase("detecting_speech")
    results = model.generate(input=str(args.audio.resolve()), batch_size=1, language="zh",
                             sentence_timestamp=True, output_timestamp=True, return_time_stamps=True)
    if len(results) != 1:
        raise ValueError(f"Expected one recording result, received {len(results)}")
    progress.phase("saving")
    bundle = normalize_result(results[0])
    bundle["metadata"] = {"model": "iic/SenseVoiceSmall", "device": "cpu", "funasr_version": version("funasr"), "elapsed_seconds": time.monotonic() - started}
    write_json(args.output, bundle)
    # Rust still needs to validate and persist the reusable checkpoint.
    progress.phase("finalizing")


if __name__ == "__main__":
    main()
