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
    args = parser.parse_args()
    started = time.monotonic()
    write_json(args.progress, {"phase": "loading_models"})
    from funasr import AutoModel
    from importlib.metadata import version
    model = AutoModel(model="iic/SenseVoiceSmall", vad_model="fsmn-vad",
                      vad_kwargs={"max_single_segment_time": 30000}, punc_model="ct-punc",
                      device="cpu", disable_update=True, disable_pbar=True)
    write_json(args.progress, {"phase": "recognizing"})
    # VAD processing does not expose a trustworthy whole-recording percentage;
    # the UI deliberately uses an indeterminate stage instead of parsing tqdm.
    results = model.generate(input=str(args.audio.resolve()), batch_size=1, language="zh",
                             sentence_timestamp=True, output_timestamp=True, return_time_stamps=True)
    if len(results) != 1:
        raise ValueError(f"Expected one recording result, received {len(results)}")
    bundle = normalize_result(results[0])
    bundle["metadata"] = {"model": "iic/SenseVoiceSmall", "device": "cpu", "funasr_version": version("funasr"), "elapsed_seconds": time.monotonic() - started}
    write_json(args.output, bundle)
    write_json(args.progress, {"phase": "complete"})


if __name__ == "__main__":
    main()
