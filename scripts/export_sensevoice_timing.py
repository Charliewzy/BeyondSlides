#!/usr/bin/env python3
"""Export SenseVoice token timing without collapsing it into subtitle rows."""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import time
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run SenseVoice with fine-grained timing and write a BeyondSlides "
            "timed-transcript sidecar."
        )
    )
    parser.add_argument("audio", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--device", default="cpu")
    parser.add_argument("--language", default="zh")
    return parser.parse_args()


def build_timed_transcript(result: dict[str, Any]) -> dict[str, list[dict[str, Any]]]:
    words = result.get("words")
    timestamps = result.get("timestamp") or result.get("timestamps")
    if not isinstance(words, list) or not isinstance(timestamps, list):
        raise ValueError("SenseVoice returned no aligned words and timestamps")
    if len(words) != len(timestamps):
        raise ValueError(
            f"SenseVoice returned {len(words)} words but {len(timestamps)} timestamps"
        )

    tokens = []
    previous_end = None
    for index, (word, timestamp) in enumerate(zip(words, timestamps, strict=True)):
        text = str(word).strip()
        if not text:
            raise ValueError(f"SenseVoice token {index} has empty text")
        if not isinstance(timestamp, (list, tuple)) or len(timestamp) < 2:
            raise ValueError(f"SenseVoice token {index} has malformed timing")
        start_ms = int(timestamp[0])
        end_ms = int(timestamp[1])
        if end_ms <= start_ms:
            raise ValueError(
                f"SenseVoice token {index} starts at {start_ms} ms and ends at {end_ms} ms"
            )
        if previous_end is not None and start_ms < previous_end:
            raise ValueError(
                f"SenseVoice token {index} overlaps the previous token ending at {previous_end} ms"
            )
        tokens.append({"text": text, "start_ms": start_ms, "end_ms": end_ms})
        previous_end = end_ms
    if not tokens:
        raise ValueError("SenseVoice returned no timed tokens")
    return {"tokens": tokens}


def write_json_atomically(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as output:
            temporary_path = Path(output.name)
            json.dump(value, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary_path, path)
    finally:
        if temporary_path is not None and temporary_path.exists():
            temporary_path.unlink()


def main() -> None:
    args = parse_args()
    if not args.audio.is_file():
        raise SystemExit(f"audio file does not exist: {args.audio}")

    from funasr import AutoModel

    model = AutoModel(
        model="iic/SenseVoiceSmall",
        vad_model="fsmn-vad",
        vad_kwargs={"max_single_segment_time": 30_000},
        punc_model="ct-punc",
        device=args.device,
        disable_update=True,
        disable_pbar=True,
    )
    started = time.monotonic()
    result = model.generate(
        input=str(args.audio),
        batch_size=1,
        language=args.language,
        sentence_timestamp=True,
        output_timestamp=True,
        return_time_stamps=True,
    )[0]
    timing = build_timed_transcript(result)
    write_json_atomically(args.output, timing)
    elapsed = time.monotonic() - started
    print(
        f"Wrote {len(timing['tokens'])} timed tokens to {args.output} "
        f"in {elapsed:.1f}s"
    )


if __name__ == "__main__":
    main()
