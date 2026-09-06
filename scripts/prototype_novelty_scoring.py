# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29", "jieba>=0.42,<0.43"]
# ///
"""PROTOTYPE: score slide-grounded novelty on fixed semantic passages."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import math
import sys
import time
import unicodedata
from collections import Counter
from pathlib import Path
from typing import Any

import jieba
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

PROMPT_VERSION = 1
BM25_K1 = 1.2
BM25_B = 0.75
SYSTEM_PROMPT = """\
你负责把大学课堂中已经固定边界的 LecturePassage 与书面幻灯片证据比较。

这一步不得改变 passage 边界或文字，也不得评价 importance。对每个 passage 独立输出：
- novelty：0=候选幻灯片直接陈述；1=基本是改述；2=有意义但大体可推知的展开；
  3=大量额外解释或细节；4=幻灯片基本没有；5=相对幻灯片真正新颖且不明显。
- connection_strength：0=无有意义联系；1=弱或偶然；2=相关但学习价值有限；
  3=清晰且有用；4=明显增进理解；5=跨概念或主题的重要综合。
- related_slides：只列出真正支持判断的零基 SlideId，不得重复。
- comparison_note：一句简洁的内部证据记录，说明哪些内容在幻灯片中出现、哪些是口头增加。

输入中的 slide_position 只是按讲课顺序推断的位置，不是对屏幕的观察，不能单独证明相关性。
candidate_slide_ids 来自该位置附近与词法检索；slides 给出本批候选页的完整文字。
只能从当前 passage 的 candidate_slide_ids 中选择 related_slides。候选页均不相关时可以返回空数组，
但仍须根据候选证据判断 novelty。不得从段落长度、表达力度或 importance 推断 novelty。

只返回 JSON，不要 Markdown 或额外解释。完整形状：
{
  "annotations": [
    {
      "passage_id": 0,
      "novelty": 3,
      "connection_strength": 2,
      "related_slides": [4],
      "comparison_note": "幻灯片给出定义；讲者补充了适用限制。"
    }
  ]
}
必须为输入中的每个 passage_id 恰好返回一项，不得增删或改变 ID。
"""


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("segmentation", type=Path)
    parser.add_argument("slides", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--passages-per-request", type=int, default=8)
    parser.add_argument("--lexical-results", type=int, default=5)
    parser.add_argument("--slide-neighborhood-radius", type=int, default=3)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--minimum-request-interval", type=float, default=5.0)
    parser.add_argument("--max-provider-retries", type=int, default=5)
    parser.add_argument("--max-json-repairs", type=int, default=3)
    parser.add_argument("--max-provider-requests", type=int, default=256)
    parser.add_argument("--canary-only", action="store_true")
    return parser.parse_args()


def analyze(text: str) -> list[str]:
    normalized = unicodedata.normalize("NFKC", text).lower()
    return [
        term
        for term in jieba.cut_for_search(normalized, HMM=True)
        if any(character.isalnum() for character in term)
    ]


class LexicalSlideIndex:
    def __init__(self, slides: list[dict[str, Any]]) -> None:
        self.slides = slides
        self.term_frequencies = [Counter(analyze(slide["text"])) for slide in slides]
        self.term_counts = [
            sum(frequencies.values()) for frequencies in self.term_frequencies
        ]
        self.document_frequencies = Counter(
            term for frequencies in self.term_frequencies for term in frequencies
        )
        self.average_document_length = sum(self.term_counts) / max(len(slides), 1)

    def search(self, query: str, limit: int) -> list[int]:
        query_terms = set(analyze(query))
        document_count = len(self.slides)
        scores = []
        for slide, frequencies, term_count in zip(
            self.slides,
            self.term_frequencies,
            self.term_counts,
            strict=True,
        ):
            score = 0.0
            for term in query_terms:
                term_frequency = frequencies.get(term, 0)
                document_frequency = self.document_frequencies.get(term, 0)
                if not term_frequency or not document_frequency:
                    continue
                inverse_document_frequency = math.log(
                    1
                    + (document_count - document_frequency + 0.5)
                    / (document_frequency + 0.5)
                )
                length_ratio = term_count / self.average_document_length
                saturation = (
                    term_frequency
                    * (BM25_K1 + 1)
                    / (term_frequency + BM25_K1 * (1 - BM25_B + BM25_B * length_ratio))
                )
                score += inverse_document_frequency * saturation
            scores.append((score, slide["id"]))
        scores.sort(key=lambda item: (-item[0], item[1]))
        return [slide_id for score, slide_id in scores[:limit] if score > 0]


def validate_inputs(
    segmentation: Any, slide_deck: Any, lexical_results: int, neighborhood_radius: int
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    passages = segmentation.get("passages") if isinstance(segmentation, dict) else None
    slides = slide_deck.get("slides") if isinstance(slide_deck, dict) else None
    if not isinstance(passages, list) or not passages:
        raise TypeError("segmentation must contain a nonempty passages array")
    if not isinstance(slides, list) or not slides:
        raise TypeError("slide deck must contain a nonempty slides array")
    if lexical_results < 0 or neighborhood_radius < 0:
        raise ValueError("retrieval sizes cannot be negative")
    for passage_id, passage in enumerate(passages):
        if passage.get("passage_id") != passage_id:
            raise ValueError("passage IDs must be zero-based and in presentation order")
        if not isinstance(passage.get("text"), str) or not passage["text"]:
            raise ValueError(f"passage {passage_id} has no text")
        if not isinstance(passage.get("slide_positions"), list) or not all(
            isinstance(position, int) for position in passage["slide_positions"]
        ):
            raise ValueError(f"passage {passage_id} has invalid slide positions")
    for slide_id, slide in enumerate(slides):
        if slide.get("id") != slide_id or not isinstance(slide.get("text"), str):
            raise ValueError("slides must be zero-based and in presentation order")
    return passages, slides


def attach_candidates(
    passages: list[dict[str, Any]],
    slides: list[dict[str, Any]],
    lexical_results: int,
    neighborhood_radius: int,
) -> None:
    index = LexicalSlideIndex(slides)
    for passage in passages:
        positions = passage["slide_positions"]
        slide_position = positions[len(positions) // 2]
        neighborhood_start = max(0, slide_position - neighborhood_radius)
        neighborhood_end = min(len(slides), slide_position + neighborhood_radius + 1)
        candidates = list(range(neighborhood_start, neighborhood_end))
        candidates.extend(index.search(passage["text"], lexical_results))
        passage["slide_position"] = slide_position
        passage["candidate_slide_ids"] = list(dict.fromkeys(candidates))


def batch_input(
    batch: list[dict[str, Any]], slides_by_id: dict[int, dict[str, Any]]
) -> str:
    visible_slide_ids = list(
        dict.fromkeys(
            slide_id for passage in batch for slide_id in passage["candidate_slide_ids"]
        )
    )
    return json.dumps(
        {
            "passages": [
                {
                    "passage_id": passage["passage_id"],
                    "text": passage["text"],
                    "slide_position": passage["slide_position"],
                    "candidate_slide_ids": passage["candidate_slide_ids"],
                }
                for passage in batch
            ],
            "slides": [slides_by_id[slide_id] for slide_id in visible_slide_ids],
        },
        ensure_ascii=False,
    )


def validate_response(
    content: str, batch: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    candidate = json.loads(strip_json_fence(content))
    if not isinstance(candidate, dict) or set(candidate) != {"annotations"}:
        raise ValueError("top-level JSON must contain only annotations")
    annotations = candidate["annotations"]
    if not isinstance(annotations, list):
        raise TypeError("annotations must be an array")
    expected = {passage["passage_id"]: passage for passage in batch}
    actual = {}
    required = {
        "passage_id",
        "novelty",
        "connection_strength",
        "related_slides",
        "comparison_note",
    }
    for annotation in annotations:
        if not isinstance(annotation, dict) or set(annotation) != required:
            raise ValueError(
                "every annotation must contain exactly the required fields"
            )
        passage_id = annotation["passage_id"]
        if not isinstance(passage_id, int) or passage_id not in expected:
            raise ValueError(f"unknown passage_id {passage_id!r}")
        if passage_id in actual:
            raise ValueError(f"passage {passage_id} was returned more than once")
        for field in ("novelty", "connection_strength"):
            score = annotation[field]
            if not isinstance(score, int) or not 0 <= score <= 5:
                raise ValueError(f"passage {passage_id} has invalid {field}")
        related_slides = annotation["related_slides"]
        if not isinstance(related_slides, list) or not all(
            isinstance(slide_id, int) for slide_id in related_slides
        ):
            raise TypeError(f"passage {passage_id} related_slides must be integers")
        if len(set(related_slides)) != len(related_slides):
            raise ValueError(f"passage {passage_id} repeats a related slide")
        allowed = expected[passage_id]["candidate_slide_ids"]
        unknown = [slide_id for slide_id in related_slides if slide_id not in allowed]
        if unknown:
            raise ValueError(
                f"passage {passage_id} selected slides {unknown} outside candidates {allowed}"
            )
        if (
            not isinstance(annotation["comparison_note"], str)
            or not annotation["comparison_note"].strip()
        ):
            raise ValueError(f"passage {passage_id} needs a comparison_note")
        actual[passage_id] = annotation
    if set(actual) != set(expected):
        raise ValueError(
            f"response omitted passages {sorted(set(expected) - set(actual))}"
        )
    return [actual[passage["passage_id"]] for passage in batch]


async def score_batch(
    batch_index: int,
    batch: list[dict[str, Any]],
    slides_by_id: dict[int, dict[str, Any]],
    client: Any,
    provider: dict[str, Any],
    budget: RequestBudget,
    limiter: RequestRateLimiter,
    arguments: argparse.Namespace,
) -> dict[str, Any]:
    started = time.perf_counter()
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": batch_input(batch, slides_by_id)},
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
            annotations = validate_response(content, batch)
            return {
                "format_version": 1,
                "batch_index": batch_index,
                "passage_ids": [passage["passage_id"] for passage in batch],
                "annotations": annotations,
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
                            f"上一份 JSON 无效：{error}。请修复它，完整回答原来的 "
                            "passages，只返回指定 JSON。"
                        ),
                    },
                ]
            )
    raise AssertionError("repair loop did not return")


async def run_batches(
    passage_batches: list[list[dict[str, Any]]],
    slides: list[dict[str, Any]],
    output_directory: Path,
    provider: dict[str, Any],
    arguments: argparse.Namespace,
) -> list[dict[str, Any]]:
    checkpoints: list[dict[str, Any] | None] = [None] * len(passage_batches)
    pending = []
    for batch_index, batch in enumerate(passage_batches):
        path = output_directory / f"novelty-batch-{batch_index:04}.json"
        if path.exists():
            checkpoint = read_json(path)
            if checkpoint.get("passage_ids") != [
                passage["passage_id"] for passage in batch
            ]:
                raise ValueError(f"checkpoint {path} does not match its passages")
            checkpoints[batch_index] = checkpoint
        else:
            pending.append((batch_index, batch))
    if arguments.canary_only:
        pending = pending[:1]

    budget = RequestBudget(arguments.max_provider_requests)
    limiter = RequestRateLimiter(arguments.minimum_request_interval)
    semaphore = asyncio.Semaphore(arguments.concurrency)
    progress_lock = asyncio.Lock()
    completed = len(passage_batches) - sum(item is None for item in checkpoints)
    slides_by_id = {slide["id"]: slide for slide in slides}

    import httpx

    async with httpx.AsyncClient(
        timeout=httpx.Timeout(240.0, connect=30.0),
        limits=httpx.Limits(max_connections=arguments.concurrency),
    ) as client:

        async def run_one(batch_index: int, batch: list[dict[str, Any]]) -> None:
            nonlocal completed
            async with semaphore:
                checkpoint = await score_batch(
                    batch_index,
                    batch,
                    slides_by_id,
                    client,
                    provider,
                    budget,
                    limiter,
                    arguments,
                )
                write_json_atomically(
                    output_directory / f"novelty-batch-{batch_index:04}.json",
                    checkpoint,
                )
                checkpoints[batch_index] = checkpoint
                async with progress_lock:
                    completed += 1
                    print(
                        f"[{completed:>3}/{len(passage_batches)}] "
                        f"novelty batch {batch_index:04} · "
                        f"{checkpoint['diagnostics']['elapsed_seconds']:.1f}s · "
                        f"{checkpoint['diagnostics']['json_repairs']} repairs",
                        flush=True,
                    )

        await asyncio.gather(*(run_one(index, batch) for index, batch in pending))
    return [checkpoint for checkpoint in checkpoints if checkpoint is not None]


def aggregate(
    passages: list[dict[str, Any]], checkpoints: list[dict[str, Any]]
) -> dict[str, Any]:
    annotations = [
        annotation
        for checkpoint in checkpoints
        for annotation in checkpoint["annotations"]
    ]
    annotations.sort(key=lambda annotation: annotation["passage_id"])
    if [annotation["passage_id"] for annotation in annotations] != list(
        range(len(passages))
    ):
        raise ValueError("novelty annotations do not cover every passage in order")
    diagnostics = [checkpoint["diagnostics"] for checkpoint in checkpoints]
    return {
        "format_version": 1,
        "annotations": annotations,
        "diagnostics": {
            "passage_count": len(passages),
            "novelty_distribution": dict(
                sorted(
                    Counter(annotation["novelty"] for annotation in annotations).items()
                )
            ),
            "connection_strength_distribution": dict(
                sorted(
                    Counter(
                        annotation["connection_strength"] for annotation in annotations
                    ).items()
                )
            ),
            "provider_calls": sum(item["provider_calls"] for item in diagnostics),
            "json_repairs": sum(item["json_repairs"] for item in diagnostics),
            "http_seconds": sum(item["http_seconds"] for item in diagnostics),
        },
    }


def main() -> int:
    arguments = parse_arguments()
    segmentation = read_json(arguments.segmentation)
    slide_deck = read_json(arguments.slides)
    passages, slides = validate_inputs(
        segmentation,
        slide_deck,
        arguments.lexical_results,
        arguments.slide_neighborhood_radius,
    )
    attach_candidates(
        passages,
        slides,
        arguments.lexical_results,
        arguments.slide_neighborhood_radius,
    )
    passage_batches = batches(passages, arguments.passages_per_request)
    base_url = required_environment("BEYOND_SLIDES_API_BASE_URL").rstrip("/")
    provider = {
        "url": f"{base_url}/chat/completions",
        "api_key": required_environment("BEYOND_SLIDES_API_KEY"),
        "model": required_environment("BEYOND_SLIDES_MODEL"),
        "extra_body": optional_extra_body(),
    }
    manifest = {
        "format_version": 1,
        "prompt_version": PROMPT_VERSION,
        "prompt_sha256": hashlib.sha256(SYSTEM_PROMPT.encode()).hexdigest(),
        "segmentation_sha256": sha256(arguments.segmentation),
        "slides_sha256": sha256(arguments.slides),
        "model": provider["model"],
        "api_base_url": base_url,
        "extra_body": provider["extra_body"],
        "passage_count": len(passages),
        "passages_per_request": arguments.passages_per_request,
        "batch_count": len(passage_batches),
        "lexical_results": arguments.lexical_results,
        "slide_neighborhood_radius": arguments.slide_neighborhood_radius,
        "concurrency": arguments.concurrency,
        "minimum_request_interval": arguments.minimum_request_interval,
        "max_provider_retries": arguments.max_provider_retries,
        "max_json_repairs": arguments.max_json_repairs,
        "max_provider_requests": arguments.max_provider_requests,
    }
    arguments.output_directory.mkdir(parents=True, exist_ok=True)
    manifest_path = arguments.output_directory / "novelty-manifest.json"
    if manifest_path.exists() and read_json(manifest_path) != manifest:
        raise ValueError(f"existing {manifest_path} does not match this experiment")
    write_json_atomically(manifest_path, manifest)

    started = time.perf_counter()
    checkpoints = asyncio.run(
        run_batches(
            passage_batches, slides, arguments.output_directory, provider, arguments
        )
    )
    if len(checkpoints) != len(passage_batches):
        print(
            f"Canary complete: {len(checkpoints)}/{len(passage_batches)} batches checkpointed."
        )
        return 0
    output = aggregate(passages, checkpoints)
    output["diagnostics"]["last_invocation_wall_seconds"] = (
        time.perf_counter() - started
    )
    path = arguments.output_directory / "novelty.json"
    write_json_atomically(path, output)
    print(f"Wrote {len(output['annotations'])} novelty annotations to {path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, TypeError, ValueError, RuntimeError) as error:
        print(f"novelty scoring prototype: {error}", file=sys.stderr)
        raise SystemExit(1) from error
