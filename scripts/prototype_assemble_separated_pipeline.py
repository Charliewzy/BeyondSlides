# /// script
# requires-python = ">=3.12"
# ///
"""PROTOTYPE: assemble independently segmented and scored lecture passages."""

from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path
from typing import Any


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_analysis", type=Path)
    parser.add_argument("segmentation", type=Path)
    parser.add_argument("importance", type=Path)
    parser.add_argument("novelty", type=Path)
    parser.add_argument("output_directory", type=Path)
    return parser.parse_args()


def read_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as source:
        return json.load(source)


def write_json(path: Path, value: Any) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def restored_text_and_spans(
    restored_transcript: dict[str, Any],
) -> tuple[str, list[dict[str, Any]]]:
    text_parts = []
    text_spans = []
    next_character = 0
    for span in restored_transcript["spans"]:
        if span["kind"] != "text":
            continue
        text = span["text"]
        end_character = next_character + len(text)
        text_parts.append(text)
        text_spans.append(
            {
                "start": next_character,
                "end": end_character,
                "source_start": span["source_start"],
                "source_end": span["source_end"],
            }
        )
        next_character = end_character
    return "".join(text_parts), text_spans


def provenance(spans: list[dict[str, Any]], start: int, end: int) -> tuple[int, int]:
    overlapping = [
        span for span in spans if start < span["end"] and end > span["start"]
    ]
    if not overlapping:
        raise ValueError(
            f"passage range {start}..{end} has no restored source provenance"
        )
    return overlapping[0]["source_start"], overlapping[-1]["source_end"]


def importance_level(percentile: float) -> int:
    return min(5, int(percentile // 20) + 1)


def main() -> int:
    arguments = parse_arguments()
    source_analysis = read_json(arguments.source_analysis)
    segmentation = read_json(arguments.segmentation)
    importance = read_json(arguments.importance)
    novelty = read_json(arguments.novelty)
    segmented = segmentation["passages"]
    importance_by_id = {
        passage["passage_id"]: passage for passage in importance["passages"]
    }
    novelty_by_id = {
        annotation["passage_id"]: annotation for annotation in novelty["annotations"]
    }
    expected_ids = set(range(len(segmented)))
    if set(importance_by_id) != expected_ids or set(novelty_by_id) != expected_ids:
        raise ValueError(
            "importance and novelty results must cover every semantic passage"
        )

    restored_text, restored_spans = restored_text_and_spans(
        source_analysis["restored_transcript"]
    )
    if "".join(passage["text"] for passage in segmented) != restored_text:
        raise ValueError(
            "semantic passages do not partition the authoritative restored transcript"
        )

    passages = []
    complete_results = []
    next_character = 0
    for passage_id, semantic_passage in enumerate(segmented):
        text = semantic_passage["text"]
        end_character = next_character + len(text)
        source_start, source_end = provenance(
            restored_spans, next_character, end_character
        )
        importance_result = importance_by_id[passage_id]
        novelty_result = novelty_by_id[passage_id]
        positions = semantic_passage["slide_positions"]
        slide_position = positions[len(positions) // 2]
        display_importance = importance_level(
            importance_result["importance_percentile"]
        )
        passages.append(
            {
                "text": text,
                "source_start": source_start,
                "source_end": source_end,
                "slide_position": slide_position,
                "novelty": novelty_result["novelty"],
                "importance": display_importance,
                "related_slides": novelty_result["related_slides"],
                "summary": None,
                "comparison_note": novelty_result["comparison_note"],
            }
        )
        complete_results.append(
            {
                "passage_id": passage_id,
                "text": text,
                "source_start": source_start,
                "source_end": source_end,
                "slide_position": slide_position,
                "source_windows": semantic_passage["source_windows"],
                "importance_best_worst": importance_result["best_worst_score"],
                "importance_percentile": importance_result["importance_percentile"],
                "importance_display_level": display_importance,
                **novelty_result,
            }
        )
        next_character = end_character

    artifact = {
        "restored_transcript": source_analysis["restored_transcript"],
        "passages": passages,
        "window_diagnostics": [],
        "window_projections": [],
    }
    importance_distribution = Counter(passage["importance"] for passage in passages)
    summary = {
        "format_version": 1,
        "question": (
            "Does importance-neutral semantic segmentation followed by independent "
            "comparative importance and slide-grounded novelty produce a more useful lecture?"
        ),
        "old_passage_count": len(source_analysis["passages"]),
        "new_passage_count": len(passages),
        "semantic_segmentation": segmentation["diagnostics"],
        "comparative_importance": importance["metrics"],
        "novelty": novelty["diagnostics"],
        "importance_display_distribution": dict(
            sorted(importance_distribution.items())
        ),
        "note": (
            "importance_display_level is a percentile presentation band; the comparative "
            "best-worst score and percentile remain authoritative for this prototype."
        ),
    }
    arguments.output_directory.mkdir(parents=True, exist_ok=True)
    write_json(arguments.output_directory / "analysis.json", artifact)
    write_json(arguments.output_directory / "pipeline-results.json", complete_results)
    write_json(arguments.output_directory / "pipeline-summary.json", summary)
    print(
        f"Wrote {len(passages)} separated-pipeline passages to "
        f"{arguments.output_directory / 'analysis.json'}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
