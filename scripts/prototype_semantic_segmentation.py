# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""PROTOTYPE: segment restored lecture text without scoring its importance."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import sys
import time
from difflib import SequenceMatcher
from itertools import pairwise
from pathlib import Path
from typing import Any

from compare_importance_bws import (
    RequestBudget,
    RequestRateLimiter,
    batches,
    optional_extra_body,
    provider_chat,
    read_json,
    required_environment,
    sha256,
    strip_json_fence,
    write_json_atomically,
)

PROMPT_VERSION = 3
MAX_CHANGED_PERCENT = 5
MAX_PASSAGE_CHARACTERS = 500
SYSTEM_PROMPT = """\
你负责把已经恢复为可读文字的大学课堂讲稿划分成语义完整的 LecturePassage。

这一步只决定语义边界。不得评价或猜测 importance 或 novelty，
也不得根据内容看起来是否重要来决定段落长短。

每个输入 window 包含 left_context、owned_text 和 right_context。必须遵守：
- 只划分 owned_text；上下文只用于理解窗口边缘是否延续同一个意思。
- passages 按顺序直接拼接后必须与 owned_text 逐字符相同。不得增删、改写或规范化空白。
- 一个 passage 应是适合学习和复习的完整讲课单元，例如一个主张及其解释、一个概念及其限制、
  一个例子及其结论，或者一段完整的代码行为说明。
- 不要把提问与紧随其后的回答拆开，不要把例子与它解释的概念拆开，也不要产生只有过渡词、
  半句话或孤立设问的 passage。
- 一个 passage 只承载一个主要教学动作。即使仍属同一大主题，当讲者从定义转入独立例子、从一个
  理由转入另一个理由、从问题转入新的子问题，或从概念转入实践建议时，也应建立自然边界。
- passage 通常为 80–250 个中文字符。内容本身需要时可以更短或更长，但不得超过 450 个字符，
  除非单段不可分割的代码讲解确实无法自然切开。不要只是为了长度均匀而切分。
- window 边缘不代表语义边界。如果 owned_text 的开头在语法上或论证上直接完成 left_context 中
  尚未结束的讲课单元，continues_previous 必须为 true；仅仅属于同一个大主题不足以标记延续。
  除第一个片段外，passages 中的每项自然开始新段。
- 每个 passage 必须非空。必须为输入中的每个 window 恰好返回一项，不得更改 window_index。

只返回 JSON，不要 Markdown 或解释。完整形状：
{
  "windows": [
    {
      "window_index": 0,
      "continues_previous": false,
      "passages": ["第一段原文。", "第二段原文。"]
    }
  ]
}
"""


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("analysis_run_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--source-windows-per-region", type=int, default=8)
    parser.add_argument("--regions-per-request", type=int, default=1)
    parser.add_argument("--context-characters", type=int, default=300)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--minimum-request-interval", type=float, default=5.0)
    parser.add_argument("--max-provider-retries", type=int, default=5)
    parser.add_argument("--max-json-repairs", type=int, default=3)
    parser.add_argument("--max-provider-requests", type=int, default=256)
    parser.add_argument("--canary-only", action="store_true")
    return parser.parse_args()


def load_source_windows(run_directory: Path) -> list[dict[str, Any]]:
    paths = sorted(run_directory.glob("window-*.json"))
    if not paths:
        raise ValueError(f"no window checkpoints found in {run_directory}")
    complete_passages = read_json(run_directory / "analysis.json").get("passages")
    if not isinstance(complete_passages, list):
        raise TypeError(f"{run_directory / 'analysis.json'} has no passages array")
    next_complete_passage = 0
    windows = []
    for window_index, path in enumerate(paths):
        checkpoint = read_json(path)
        passages = checkpoint.get("analysis", {}).get("passages")
        if not isinstance(passages, list) or not passages:
            raise ValueError(f"{path} has no analyzed passages")
        if not all(isinstance(passage.get("text"), str) for passage in passages):
            raise ValueError(f"{path} contains a passage without text")
        matching_complete = complete_passages[
            next_complete_passage : next_complete_passage + len(passages)
        ]
        if len(matching_complete) != len(passages) or any(
            stored.get("text") != complete.get("text")
            for stored, complete in zip(passages, matching_complete, strict=True)
        ):
            raise ValueError(
                f"{path} does not match the assembled analysis in presentation order"
            )
        next_complete_passage += len(passages)
        slide_positions = {
            passage.get("slide_position") for passage in matching_complete
        }
        if len(slide_positions) != 1 or not all(
            isinstance(position, int) for position in slide_positions
        ):
            raise ValueError(f"{path} does not have one integer slide position")
        slide_position = next(iter(slide_positions))
        text = "".join(passage["text"] for passage in passages)
        if not text:
            raise ValueError(f"{path} has empty owned text")
        spans = []
        next_character = 0
        for passage in passages:
            end_character = next_character + len(passage["text"])
            spans.append(
                {
                    "start": next_character,
                    "end": end_character,
                    "source_start": passage["source_start"],
                    "source_end": passage["source_end"],
                    "slide_position": slide_position,
                    "source_window": window_index,
                }
            )
            next_character = end_character
        windows.append(
            {
                "window_index": window_index,
                "text": text,
                "slide_position": slide_position,
                "source_spans": spans,
            }
        )
    if next_complete_passage != len(complete_passages):
        raise ValueError("window checkpoints do not cover the assembled analysis passages")
    return windows


def coalesce_source_windows(
    source_windows: list[dict[str, Any]], windows_per_region: int
) -> list[dict[str, Any]]:
    if windows_per_region <= 0:
        raise ValueError("source windows per region must be greater than zero")
    regions = []
    for region_index, members in enumerate(batches(source_windows, windows_per_region)):
        text_parts = []
        source_spans = []
        next_character = 0
        for member in members:
            text_parts.append(member["text"])
            for span in member["source_spans"]:
                source_spans.append(
                    {
                        **span,
                        "start": span["start"] + next_character,
                        "end": span["end"] + next_character,
                    }
                )
            next_character += len(member["text"])
        regions.append(
            {
                "window_index": region_index,
                "text": "".join(text_parts),
                "source_spans": source_spans,
                "source_windows": [member["window_index"] for member in members],
            }
        )
    return regions


def add_context(windows: list[dict[str, Any]], characters: int) -> None:
    if characters < 0:
        raise ValueError("context characters cannot be negative")
    for index, window in enumerate(windows):
        left = windows[index - 1]["text"] if index > 0 else ""
        right = windows[index + 1]["text"] if index + 1 < len(windows) else ""
        window["left_context"] = left[-characters:] if characters else ""
        window["right_context"] = right[:characters] if characters else ""


def batch_input(batch: list[dict[str, Any]]) -> str:
    return json.dumps(
        {
            "windows": [
                {
                    "window_index": window["window_index"],
                    "left_context": window["left_context"],
                    "owned_text": window["text"],
                    "right_context": window["right_context"],
                }
                for window in batch
            ]
        },
        ensure_ascii=False,
    )


def project_passages(source: str, proposed: list[str]) -> tuple[list[str], dict[str, Any]]:
    if not proposed or not all(isinstance(text, str) and text for text in proposed):
        raise ValueError("passages must be a nonempty array of nonempty strings")
    copied = "".join(proposed)
    boundaries = [0]
    for text in proposed:
        boundaries.append(boundaries[-1] + len(text))
    if copied == source:
        return proposed, {
            "changed_characters": 0,
            "compared_characters": len(source),
        }

    operations = SequenceMatcher(None, source, copied, autojunk=False).get_opcodes()
    changed = sum(
        max(source_end - source_start, copied_end - copied_start)
        for tag, source_start, source_end, copied_start, copied_end in operations
        if tag != "equal"
    )
    compared = max(len(source), len(copied))
    if changed * 100 >= compared * MAX_CHANGED_PERCENT:
        raise ValueError(
            f"copied text changed {changed}/{compared} characters, reaching the 5% limit"
        )

    source_positions = [0] * (len(copied) + 1)
    next_unprojected = 0
    for _, source_start, source_end, copied_start, copied_end in operations:
        source_length = source_end - source_start
        copied_length = copied_end - copied_start
        for offset in range(copied_length + 1):
            copied_position = copied_start + offset
            if copied_position < next_unprojected:
                continue
            source_offset = (offset * source_length + copied_length // 2) // max(
                copied_length, 1
            )
            source_positions[copied_position] = source_start + source_offset
            next_unprojected = copied_position + 1
    source_positions[0] = 0
    source_positions[len(copied)] = len(source)

    projected = []
    for passage_index, (start, end) in enumerate(pairwise(boundaries)):
        source_start = source_positions[start]
        source_end = source_positions[end]
        if source_start >= source_end:
            raise ValueError(f"passage {passage_index} projects to an empty range")
        projected.append(source[source_start:source_end])
    if "".join(projected) != source:
        raise AssertionError("projected passages do not partition authoritative text")
    return projected, {
        "changed_characters": changed,
        "compared_characters": compared,
    }


def validate_response(
    content: str, batch: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    candidate = json.loads(strip_json_fence(content))
    if not isinstance(candidate, dict) or set(candidate) != {"windows"}:
        raise ValueError("top-level JSON must contain only windows")
    proposed_windows = candidate["windows"]
    if not isinstance(proposed_windows, list):
        raise TypeError("windows must be an array")
    expected = {window["window_index"]: window for window in batch}
    actual = {}
    for proposed_window in proposed_windows:
        if not isinstance(proposed_window, dict) or set(proposed_window) != {
            "window_index",
            "continues_previous",
            "passages",
        }:
            raise ValueError("every window must contain exactly the three required fields")
        window_index = proposed_window["window_index"]
        if not isinstance(window_index, int) or window_index not in expected:
            raise ValueError(f"unknown window_index {window_index!r}")
        if window_index in actual:
            raise ValueError(f"window {window_index} was returned more than once")
        if not isinstance(proposed_window["continues_previous"], bool):
            raise TypeError(f"window {window_index} continues_previous must be Boolean")
        if window_index == 0 and proposed_window["continues_previous"]:
            raise ValueError("the first lecture window cannot continue a previous passage")
        passages, projection = project_passages(
            expected[window_index]["text"], proposed_window["passages"]
        )
        oversized = [
            (passage_index, len(text))
            for passage_index, text in enumerate(passages)
            if len(text) > MAX_PASSAGE_CHARACTERS
        ]
        if oversized:
            passage_index, character_count = oversized[0]
            raise ValueError(
                f"window {window_index} passage {passage_index} contains "
                f"{character_count} characters; split it at a semantic boundary so every "
                f"passage contains at most {MAX_PASSAGE_CHARACTERS}"
            )
        actual[window_index] = {
            "window_index": window_index,
            "continues_previous": proposed_window["continues_previous"],
            "passages": passages,
            "projection": projection,
        }
    if set(actual) != set(expected):
        raise ValueError(f"response omitted windows {sorted(set(expected) - set(actual))}")
    return [actual[window["window_index"]] for window in batch]


async def segment_batch(
    batch_index: int,
    batch: list[dict[str, Any]],
    client: Any,
    provider: dict[str, Any],
    budget: RequestBudget,
    limiter: RequestRateLimiter,
    arguments: argparse.Namespace,
) -> dict[str, Any]:
    started = time.perf_counter()
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": batch_input(batch)},
    ]
    raw_responses = []
    prompt_tokens = 0
    completion_tokens = 0
    provider_attempts = 0
    http_seconds = 0.0
    token_counts_complete = True
    for repair in range(arguments.max_json_repairs + 1):
        content, usage, diagnostics = await provider_chat(
            client,
            provider["url"],
            provider["api_key"],
            provider["model"],
            provider["extra_body"],
            messages,
            budget,
            limiter,
            arguments.max_provider_retries,
        )
        provider_attempts += diagnostics["provider_attempts"]
        http_seconds += diagnostics["http_seconds"]
        raw_responses.append(
            {
                "repair": repair,
                "response_id": diagnostics["response_id"],
                "content": content,
            }
        )
        if isinstance(usage.get("prompt_tokens"), int) and isinstance(
            usage.get("completion_tokens"), int
        ):
            prompt_tokens += usage["prompt_tokens"]
            completion_tokens += usage["completion_tokens"]
        else:
            token_counts_complete = False
        try:
            windows = validate_response(content, batch)
            return {
                "format_version": 1,
                "batch_index": batch_index,
                "source_window_indices": [window["window_index"] for window in batch],
                "windows": windows,
                "diagnostics": {
                    "elapsed_seconds": time.perf_counter() - started,
                    "http_seconds": http_seconds,
                    "provider_calls": repair + 1,
                    "provider_attempts": provider_attempts,
                    "json_repairs": repair,
                    "prompt_tokens": prompt_tokens if token_counts_complete else None,
                    "completion_tokens": completion_tokens
                    if token_counts_complete
                    else None,
                },
                "raw_responses": raw_responses,
            }
        except (json.JSONDecodeError, TypeError, ValueError) as error:
            if repair == arguments.max_json_repairs:
                raise RuntimeError(
                    f"batch {batch_index} remained invalid after {repair} repairs: {error}"
                ) from error
            messages.extend(
                [
                    {"role": "assistant", "content": content},
                    {
                        "role": "user",
                        "content": (
                            f"上一份 JSON 无效：{error}。请修复它，完整回答原来的所有 "
                            "windows，只返回指定 JSON。"
                        ),
                    },
                ]
            )
    raise AssertionError("repair loop did not return")


def load_checkpoint(path: Path, batch: list[dict[str, Any]]) -> dict[str, Any]:
    checkpoint = read_json(path)
    expected_indices = [window["window_index"] for window in batch]
    if checkpoint.get("source_window_indices") != expected_indices:
        raise ValueError(f"checkpoint {path} does not match its source windows")
    return checkpoint


async def run_batches(
    source_batches: list[list[dict[str, Any]]],
    output_directory: Path,
    provider: dict[str, Any],
    arguments: argparse.Namespace,
) -> list[dict[str, Any]]:
    checkpoints: list[dict[str, Any] | None] = [None] * len(source_batches)
    pending = []
    for batch_index, batch in enumerate(source_batches):
        path = output_directory / f"segmentation-batch-{batch_index:04}.json"
        if path.exists():
            checkpoints[batch_index] = load_checkpoint(path, batch)
        else:
            pending.append((batch_index, batch))
    if arguments.canary_only:
        pending = pending[:1]

    budget = RequestBudget(arguments.max_provider_requests)
    limiter = RequestRateLimiter(arguments.minimum_request_interval)
    semaphore = asyncio.Semaphore(arguments.concurrency)
    progress_lock = asyncio.Lock()
    completed = len(source_batches) - sum(item is None for item in checkpoints)

    import httpx

    async with httpx.AsyncClient(
        timeout=httpx.Timeout(240.0, connect=30.0),
        limits=httpx.Limits(max_connections=arguments.concurrency),
    ) as client:

        async def run_one(batch_index: int, batch: list[dict[str, Any]]) -> None:
            nonlocal completed
            async with semaphore:
                checkpoint = await segment_batch(
                    batch_index,
                    batch,
                    client,
                    provider,
                    budget,
                    limiter,
                    arguments,
                )
                write_json_atomically(
                    output_directory / f"segmentation-batch-{batch_index:04}.json",
                    checkpoint,
                )
                checkpoints[batch_index] = checkpoint
                async with progress_lock:
                    completed += 1
                    print(
                        f"[{completed:>3}/{len(source_batches)}] "
                        f"segmentation batch {batch_index:04} · "
                        f"{checkpoint['diagnostics']['elapsed_seconds']:.1f}s · "
                        f"{checkpoint['diagnostics']['json_repairs']} repairs",
                        flush=True,
                    )

        await asyncio.gather(*(run_one(index, batch) for index, batch in pending))
    return [checkpoint for checkpoint in checkpoints if checkpoint is not None]


def provenance(window: dict[str, Any], start: int, end: int) -> dict[str, Any]:
    overlapping = [
        span
        for span in window["source_spans"]
        if start < span["end"] and end > span["start"]
    ]
    if not overlapping:
        raise ValueError(
            f"projected passage {start}..{end} has no provenance in window "
            f"{window['window_index']}"
        )
    return {
        "source_start": overlapping[0]["source_start"],
        "source_end": overlapping[-1]["source_end"],
        "slide_positions": list(
            dict.fromkeys(span["slide_position"] for span in overlapping)
        ),
        "source_windows": list(
            dict.fromkeys(span["source_window"] for span in overlapping)
        ),
    }


def assemble(
    source_windows: list[dict[str, Any]], checkpoints: list[dict[str, Any]]
) -> dict[str, Any]:
    proposed_windows = [
        window for checkpoint in checkpoints for window in checkpoint["windows"]
    ]
    proposed_windows.sort(key=lambda window: window["window_index"])
    if len(proposed_windows) != len(source_windows):
        raise ValueError("segmentation is incomplete")

    passages = []
    fuzzy_window_count = 0
    for source_window, proposed_window in zip(
        source_windows, proposed_windows, strict=True
    ):
        if source_window["window_index"] != proposed_window["window_index"]:
            raise ValueError("segmentation windows are out of order")
        if proposed_window["projection"]["changed_characters"]:
            fuzzy_window_count += 1
        next_character = 0
        pieces = []
        for text in proposed_window["passages"]:
            end_character = next_character + len(text)
            source = provenance(source_window, next_character, end_character)
            pieces.append(
                {
                    "text": text,
                    **source,
                }
            )
            next_character = end_character
        if proposed_window["continues_previous"]:
            if not passages:
                raise ValueError("first semantic passage cannot continue a previous window")
            first = pieces.pop(0)
            passages[-1]["text"] += first["text"]
            passages[-1]["source_end"] = first["source_end"]
            passages[-1]["slide_positions"].extend(first["slide_positions"])
            passages[-1]["source_windows"].extend(first["source_windows"])
        passages.extend(pieces)

    for passage_id, passage in enumerate(passages):
        passage["passage_id"] = passage_id
        passage["slide_positions"] = list(dict.fromkeys(passage["slide_positions"]))
    lengths = [len(passage["text"]) for passage in passages]
    diagnostics = [checkpoint["diagnostics"] for checkpoint in checkpoints]
    return {
        "format_version": 1,
        "passages": passages,
        "diagnostics": {
            "segmentation_region_count": len(source_windows),
            "source_window_count": len(
                {
                    source_window
                    for passage in passages
                    for source_window in passage["source_windows"]
                }
            ),
            "passage_count": len(passages),
            "continued_window_count": sum(
                window["continues_previous"] for window in proposed_windows
            ),
            "fuzzy_window_count": fuzzy_window_count,
            "minimum_characters": min(lengths),
            "median_characters": sorted(lengths)[len(lengths) // 2],
            "maximum_characters": max(lengths),
            "provider_calls": sum(item["provider_calls"] for item in diagnostics),
            "json_repairs": sum(item["json_repairs"] for item in diagnostics),
            "http_seconds": sum(item["http_seconds"] for item in diagnostics),
        },
    }


def main() -> int:
    arguments = parse_arguments()
    original_windows = load_source_windows(arguments.analysis_run_directory)
    source_windows = coalesce_source_windows(
        original_windows, arguments.source_windows_per_region
    )
    add_context(source_windows, arguments.context_characters)
    source_batches = batches(source_windows, arguments.regions_per_request)
    base_url = required_environment("BEYOND_SLIDES_API_BASE_URL").rstrip("/")
    provider = {
        "url": f"{base_url}/chat/completions",
        "api_key": required_environment("BEYOND_SLIDES_API_KEY"),
        "model": required_environment("BEYOND_SLIDES_MODEL"),
        "extra_body": optional_extra_body(),
    }
    source_analysis = arguments.analysis_run_directory / "analysis.json"
    manifest = {
        "format_version": 1,
        "prompt_version": PROMPT_VERSION,
        "prompt_sha256": hashlib.sha256(SYSTEM_PROMPT.encode()).hexdigest(),
        "source_analysis_sha256": sha256(source_analysis),
        "model": provider["model"],
        "api_base_url": base_url,
        "extra_body": provider["extra_body"],
        "source_window_count": len(original_windows),
        "segmentation_region_count": len(source_windows),
        "source_windows_per_region": arguments.source_windows_per_region,
        "regions_per_request": arguments.regions_per_request,
        "context_characters": arguments.context_characters,
        "concurrency": arguments.concurrency,
        "minimum_request_interval": arguments.minimum_request_interval,
        "max_provider_retries": arguments.max_provider_retries,
        "max_json_repairs": arguments.max_json_repairs,
        "max_provider_requests": arguments.max_provider_requests,
    }
    arguments.output_directory.mkdir(parents=True, exist_ok=True)
    manifest_path = arguments.output_directory / "segmentation-manifest.json"
    if manifest_path.exists() and read_json(manifest_path) != manifest:
        raise ValueError(f"existing {manifest_path} does not match this experiment")
    write_json_atomically(manifest_path, manifest)

    started = time.perf_counter()
    checkpoints = asyncio.run(
        run_batches(source_batches, arguments.output_directory, provider, arguments)
    )
    if len(checkpoints) != len(source_batches):
        print(
            f"Canary complete: {len(checkpoints)}/{len(source_batches)} batches checkpointed."
        )
        return 0
    output = assemble(source_windows, checkpoints)
    output["diagnostics"]["last_invocation_wall_seconds"] = time.perf_counter() - started
    path = arguments.output_directory / "semantic-segmentation.json"
    write_json_atomically(path, output)
    print(f"Wrote {len(output['passages'])} semantic passages to {path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, TypeError, ValueError, RuntimeError) as error:
        print(f"semantic segmentation prototype: {error}", file=sys.stderr)
        raise SystemExit(1) from error
