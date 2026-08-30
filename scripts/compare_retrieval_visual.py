#!/usr/bin/env python3
"""Compare retrieved slides with visually observed slide positions."""

from __future__ import annotations

import argparse
import bisect
import json
import statistics
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any


STABLE_DOMINANT_SHARE = 0.80


@dataclass(frozen=True)
class WindowComparison:
    number: int
    start_ms: int
    end_ms: int
    frame_count: int
    dominant_slide: int
    dominant_share: float
    observed_slides: tuple[int, ...]
    observed_slide_counts: tuple[tuple[int, int], ...]
    candidate_slides: tuple[int, ...]
    dominant_rank: int | None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Compare retrieval candidates with the slide positions observed in "
            "a visual-alignment artifact."
        )
    )
    parser.add_argument("retrieval_probe", type=Path)
    parser.add_argument("visual_alignment", type=Path)
    parser.add_argument(
        "--semantic-alignment",
        type=Path,
        help="optional semantic DP alignment artifact to evaluate",
    )
    parser.add_argument(
        "--mismatches",
        type=int,
        default=15,
        help="number of top-1 mismatches to print (default: 15)",
    )
    return parser.parse_args()


def load_json(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as file:
        value = json.load(file)
    if not isinstance(value, dict):
        raise ValueError(f"{path} does not contain a JSON object")
    return value


def compare(
    retrieval_probe: dict[str, Any], visual_alignment: dict[str, Any]
) -> tuple[list[WindowComparison], dict[int, int], int, int, list[float]]:
    frame_matches = visual_alignment["frame_matches"]
    sample_period_ms = visual_alignment["sample_period_ms"]
    timestamps = [frame["timestamp_ms"] for frame in frame_matches]
    comparisons: list[WindowComparison] = []
    frame_hits: Counter[int] = Counter()
    compared_frames = 0
    nearest_sample_windows = 0
    visual_margins: list[float] = []

    for window in retrieval_probe["windows"]:
        owned_region = window["owned_region"]
        start_ms = owned_region["start_ms"]
        end_ms = owned_region["end_ms"]
        first_frame = bisect.bisect_left(timestamps, start_ms)
        after_last_frame = bisect.bisect_right(timestamps, end_ms)
        frames = frame_matches[first_frame:after_last_frame]
        if not frames:
            midpoint_ms = (start_ms + end_ms) // 2
            insertion = bisect.bisect_left(timestamps, midpoint_ms)
            nearby_indices = [
                index
                for index in (insertion - 1, insertion)
                if 0 <= index < len(frame_matches)
            ]
            nearest_index = min(
                nearby_indices,
                key=lambda index: abs(timestamps[index] - midpoint_ms),
            )
            if abs(timestamps[nearest_index] - midpoint_ms) > sample_period_ms:
                continue
            frames = [frame_matches[nearest_index]]
            nearest_sample_windows += 1

        observed = [frame["best"]["slide_id"] for frame in frames]
        frequencies = Counter(observed)
        dominant_slide, dominant_count = frequencies.most_common(1)[0]
        candidates = tuple(candidate["slide_id"] for candidate in window["candidates"])
        dominant_rank = (
            candidates.index(dominant_slide) + 1 if dominant_slide in candidates else None
        )

        for slide_id in observed:
            for cutoff in range(1, len(candidates) + 1):
                if slide_id in candidates[:cutoff]:
                    frame_hits[cutoff] += 1
        visual_margins.extend(
            frame["best"]["score"] - frame["runner_up"]["score"]
            for frame in frames
            if frame["runner_up"] is not None
        )
        compared_frames += len(frames)
        comparisons.append(
            WindowComparison(
                number=window["number"],
                start_ms=start_ms,
                end_ms=end_ms,
                frame_count=len(frames),
                dominant_slide=dominant_slide,
                dominant_share=dominant_count / len(frames),
                observed_slides=tuple(sorted(frequencies)),
                observed_slide_counts=tuple(sorted(frequencies.items())),
                candidate_slides=candidates,
                dominant_rank=dominant_rank,
            )
        )

    return (
        comparisons,
        dict(frame_hits),
        compared_frames,
        nearest_sample_windows,
        visual_margins,
    )


def percent(numerator: int, denominator: int) -> str:
    return f"{100.0 * numerator / denominator:.1f}%" if denominator else "n/a"


def display_time(milliseconds: int) -> str:
    seconds = milliseconds // 1_000
    return f"{seconds // 3_600:02}:{(seconds // 60) % 60:02}:{seconds % 60:02}"


def print_metrics(
    comparisons: list[WindowComparison],
    frame_hits: dict[int, int],
    compared_frames: int,
    nearest_sample_windows: int,
    visual_margins: list[float],
    max_results: int,
) -> None:
    stable = [
        comparison
        for comparison in comparisons
        if comparison.dominant_share >= STABLE_DOMINANT_SHARE
    ]

    print("Displayed-slide retrieval agreement")
    print(f"Compared windows: {len(comparisons)}")
    print(f"Compared one-second frames: {compared_frames}")
    print(f"Windows using their nearest sampled frame: {nearest_sample_windows}")
    print(
        f"Stable windows (dominant slide >= {STABLE_DOMINANT_SHARE:.0%}): "
        f"{len(stable)}"
    )
    print()
    print("                  all windows   stable windows   frame-weighted")
    for cutoff in (1, 3, max_results):
        all_hits = sum(
            comparison.dominant_rank is not None
            and comparison.dominant_rank <= cutoff
            for comparison in comparisons
        )
        stable_hits = sum(
            comparison.dominant_rank is not None
            and comparison.dominant_rank <= cutoff
            for comparison in stable
        )
        print(
            f"top-{cutoff:<2} agreement   "
            f"{percent(all_hits, len(comparisons)):>8}      "
            f"{percent(stable_hits, len(stable)):>8}       "
            f"{percent(frame_hits.get(cutoff, 0), compared_frames):>8}"
        )

    reciprocal_rank = sum(
        1.0 / comparison.dominant_rank
        for comparison in comparisons
        if comparison.dominant_rank is not None
    ) / len(comparisons)
    print(f"Dominant-slide MRR@{max_results}: {reciprocal_rank:.3f}")
    if visual_margins:
        print(
            "Visual best-vs-runner-up MSSIM margin: "
            f"median={statistics.median(visual_margins):.3f}, "
            f"10th percentile={sorted(visual_margins)[len(visual_margins) // 10]:.3f}"
        )


def print_mismatches(comparisons: list[WindowComparison], limit: int) -> None:
    mismatches = [
        comparison for comparison in comparisons if comparison.dominant_rank != 1
    ]
    mismatches.sort(
        key=lambda comparison: (
            -comparison.dominant_share,
            comparison.number,
        )
    )
    print()
    print(f"Strongest top-1 mismatches (showing {min(limit, len(mismatches))}):")
    for comparison in mismatches[:limit]:
        rank = (
            str(comparison.dominant_rank)
            if comparison.dominant_rank is not None
            else "miss"
        )
        print(
            f"window {comparison.number:>3} "
            f"{display_time(comparison.start_ms)}-{display_time(comparison.end_ms)}  "
            f"shown={comparison.dominant_slide} "
            f"share={comparison.dominant_share:>5.1%} "
            f"rank={rank:<4} "
            f"retrieved={list(comparison.candidate_slides)} "
            f"observed={list(comparison.observed_slides)}"
        )


def print_semantic_alignment_metrics(
    comparisons: list[WindowComparison], semantic_alignment: dict[str, Any]
) -> None:
    windows = semantic_alignment["windows"]
    if not windows:
        raise ValueError("the semantic alignment artifact has no windows")
    slide_ids = tuple(
        score["slide_id"] for score in windows[0]["slide_scores"]
    )
    if slide_ids != tuple(range(len(slide_ids))):
        raise ValueError(
            "semantic alignment slide scores must use canonical zero-based IDs"
        )
    for window in windows:
        window_slide_ids = tuple(
            score["slide_id"] for score in window["slide_scores"]
        )
        if window_slide_ids != slide_ids:
            raise ValueError(
                "semantic alignment windows do not share one slide presentation order"
            )
        if not 0 <= window["slide_position"] < len(slide_ids):
            raise ValueError("semantic alignment selected an unknown slide ID")

    observed_slide_ids = {
        slide_id
        for comparison in comparisons
        for slide_id in (
            comparison.dominant_slide,
            *comparison.observed_slides,
        )
    }
    if any(not 0 <= slide_id < len(slide_ids) for slide_id in observed_slide_ids):
        raise ValueError("visual alignment contains an unknown slide ID")

    positions_by_window = {
        window["number"]: window["slide_position"] for window in windows
    }
    evaluated = [
        (comparison, positions_by_window[comparison.number])
        for comparison in comparisons
        if comparison.number in positions_by_window
    ]
    stable = [
        (comparison, position)
        for comparison, position in evaluated
        if comparison.dominant_share >= STABLE_DOMINANT_SHARE
    ]

    print()
    print("Semantic DP slide-position agreement")
    print("                  all windows   stable windows   frame-weighted")
    for distance in (0, 1, 3):
        all_hits = sum(
            abs(position - comparison.dominant_slide) <= distance
            for comparison, position in evaluated
        )
        stable_hits = sum(
            abs(position - comparison.dominant_slide) <= distance
            for comparison, position in stable
        )
        frame_hits = sum(
            count
            for comparison, position in evaluated
            for slide_id, count in comparison.observed_slide_counts
            if abs(position - slide_id) <= distance
        )
        compared_frames = sum(comparison.frame_count for comparison, _ in evaluated)
        label = "exact" if distance == 0 else f"within {distance}"
        print(
            f"{label:<16}  "
            f"{percent(all_hits, len(evaluated)):>8}      "
            f"{percent(stable_hits, len(stable)):>8}       "
            f"{percent(frame_hits, compared_frames):>8}"
        )

    positions = [position for _, position in evaluated]
    backward_transitions = sum(
        right < left for left, right in zip(positions, positions[1:])
    )
    distant_transitions = sum(
        abs(right - left) > 3 for left, right in zip(positions, positions[1:])
    )
    print(f"Backward transitions: {backward_transitions}")
    print(f"Transitions larger than three slides: {distant_transitions}")


def main() -> None:
    arguments = parse_args()
    retrieval_probe = load_json(arguments.retrieval_probe)
    visual_alignment = load_json(arguments.visual_alignment)
    max_results = retrieval_probe["retrieval"]["max_results"]
    (
        comparisons,
        frame_hits,
        compared_frames,
        nearest_sample_windows,
        visual_margins,
    ) = compare(
        retrieval_probe, visual_alignment
    )
    if not comparisons:
        raise ValueError("the artifacts have no overlapping transcript windows and frames")
    print_metrics(
        comparisons,
        frame_hits,
        compared_frames,
        nearest_sample_windows,
        visual_margins,
        max_results,
    )
    print_mismatches(comparisons, arguments.mismatches)
    if arguments.semantic_alignment is not None:
        print_semantic_alignment_metrics(
            comparisons, load_json(arguments.semantic_alignment)
        )


if __name__ == "__main__":
    main()
