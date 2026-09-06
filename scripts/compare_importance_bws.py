# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Run a resumable best--worst importance comparison over fixed lecture passages."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import math
import os
import random
import statistics
import sys
import time
from collections import Counter, defaultdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import httpx

PROMPT_VERSION = 1
SYSTEM_PROMPT = """\
你负责比较同一节大学课程讲稿中各段内容的学习重要性。

每个 quartet 都是独立比较题。对于每个 quartet：
1. 先分别判断四个段落对理解和复习本讲核心内容的作用。
2. 选择一个 most_important：如果学生完全遗漏它，对重建本讲概念、推理、方法、关键限制或可迁移经验的损失最大。
3. 选择一个 least_important：遗漏它造成的学习损失最小。

判断原则：
- 评价信息的中心性、解释力、可迁移性和避免误解的作用。
- 不要仅因为段落更长、措辞更权威、包含更多术语或重复强调就认为它更重要。
- 过渡语、重复、局部机械细节通常较低；但非显然的限制、反例和关键代码行为可能很高。
- 必须从每组给出的四个 passage_id 中各选一个 most 和 least，二者不能相同，不允许平局。
- 不得使用或猜测原有的重要性分数。

只返回 JSON，不要 Markdown 或解释。完整形状：
{
  "comparisons": [
    {
      "quartet_id": 0,
      "most_important": 12,
      "least_important": 34
    }
  ]
}
必须为输入中的每个 quartet 恰好返回一项，不得增删、重复或改变 quartet_id。
"""


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("analysis", type=Path)
    parser.add_argument("slides", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--rounds", type=int, default=8)
    parser.add_argument("--quartets-per-request", type=int, default=16)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--minimum-request-interval", type=float, default=5.0)
    parser.add_argument("--max-provider-retries", type=int, default=5)
    parser.add_argument("--max-json-repairs", type=int, default=2)
    parser.add_argument("--max-provider-requests", type=int, default=256)
    parser.add_argument("--seed", type=int, default=20260905)
    parser.add_argument("--canary-only", action="store_true")
    return parser.parse_args()


def read_json(path: Path) -> Any:
    with path.open(encoding="utf-8") as source:
        return json.load(source)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json_atomically(path: Path, value: Any) -> None:
    write_text_atomically(path, json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def write_text_atomically(path: Path, value: str) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(value, encoding="utf-8")
    temporary.replace(path)


def required_environment(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ValueError(f"required environment variable {name} is not set")
    return value


def optional_extra_body() -> dict[str, Any]:
    text = os.environ.get("BEYOND_SLIDES_CHAT_EXTRA_BODY")
    if not text:
        return {}
    value = json.loads(text)
    if not isinstance(value, dict):
        raise TypeError("BEYOND_SLIDES_CHAT_EXTRA_BODY must be a JSON object")
    return value


def validate_inputs(analysis: Any, slides: Any) -> list[dict[str, Any]]:
    if not isinstance(analysis, dict) or not isinstance(analysis.get("passages"), list):
        raise TypeError("analysis must contain a passages array")
    passages = analysis["passages"]
    if len(passages) < 4:
        raise ValueError("best--worst scaling requires at least four passages")
    for passage_id, passage in enumerate(passages):
        if not isinstance(passage, dict) or not isinstance(passage.get("text"), str):
            raise TypeError(f"passage {passage_id} has no text")
        importance = passage.get("importance")
        if importance is not None and (
            not isinstance(importance, int) or not 0 <= importance <= 5
        ):
            raise ValueError(f"passage {passage_id} has invalid importance")
    reference_scores = [passage.get("importance") for passage in passages]
    if any(score is None for score in reference_scores) and not all(
        score is None for score in reference_scores
    ):
        raise ValueError(
            "importance must be present on every passage or omitted from all"
        )
    if not isinstance(slides, dict) or not isinstance(slides.get("slides"), list):
        raise TypeError("slides must contain a slides array")
    return passages


def lecture_context(slides: dict[str, Any]) -> str:
    texts = [slide.get("text", "") for slide in slides["slides"][:2]]
    return "\n\n".join(text.strip() for text in texts if text.strip())[:3_000]


def build_quartets(passage_count: int, rounds: int, seed: int) -> list[dict[str, Any]]:
    if rounds < 2 or rounds % 2 != 0:
        raise ValueError("rounds must be a positive even number of at least two")
    quartets = []
    quartet_id = 0
    for round_index in range(rounds):
        passage_ids = list(range(passage_count))
        random.Random(seed + round_index).shuffle(passage_ids)
        missing = (-len(passage_ids)) % 4
        passage_ids.extend(passage_ids[:missing])
        for offset in range(0, len(passage_ids), 4):
            members = passage_ids[offset : offset + 4]
            if len(set(members)) != 4:
                raise AssertionError("a generated quartet contains a duplicate passage")
            quartets.append(
                {
                    "quartet_id": quartet_id,
                    "round": round_index,
                    "passage_ids": members,
                }
            )
            quartet_id += 1
    return quartets


def batches(items: list[Any], size: int) -> list[list[Any]]:
    if size <= 0:
        raise ValueError("quartets per request must be greater than zero")
    return [items[start : start + size] for start in range(0, len(items), size)]


class RequestBudget:
    def __init__(self, maximum: int) -> None:
        if maximum <= 0:
            raise ValueError("max provider requests must be greater than zero")
        self.maximum = maximum
        self.count = 0
        self._lock = asyncio.Lock()

    async def claim(self) -> int:
        async with self._lock:
            if self.count >= self.maximum:
                raise RuntimeError(
                    f"provider request safety limit {self.maximum} has been reached"
                )
            self.count += 1
            return self.count


class RequestRateLimiter:
    def __init__(self, minimum_interval: float) -> None:
        if minimum_interval < 0:
            raise ValueError("minimum request interval cannot be negative")
        self.minimum_interval = minimum_interval
        self._next_start = 0.0
        self._lock = asyncio.Lock()

    async def acquire(self) -> None:
        async with self._lock:
            now = asyncio.get_running_loop().time()
            scheduled = max(now, self._next_start)
            self._next_start = scheduled + self.minimum_interval
        await asyncio.sleep(max(0.0, scheduled - now))


def retry_delay(response: httpx.Response | None, retry: int) -> float:
    if response is not None:
        retry_after = response.headers.get("retry-after")
        if retry_after:
            try:
                return min(120.0, float(retry_after))
            except ValueError:
                pass
        if response.status_code == 429:
            return min(80.0, 5.0 * 2 ** min(4, retry - 1))
    return float(min(5, retry))


async def provider_chat(
    client: httpx.AsyncClient,
    url: str,
    api_key: str,
    model: str,
    extra_body: dict[str, Any],
    messages: list[dict[str, str]],
    budget: RequestBudget,
    limiter: RequestRateLimiter,
    max_retries: int,
) -> tuple[str, dict[str, Any], dict[str, Any]]:
    payload = {
        "model": model,
        "messages": messages,
        "temperature": 0,
        "max_tokens": 8_192,
        "response_format": {"type": "json_object"},
        **extra_body,
    }
    errors = []
    total_http_seconds = 0.0
    for provider_attempt in range(max_retries + 1):
        request_number = await budget.claim()
        await limiter.acquire()
        started = time.perf_counter()
        response = None
        try:
            response = await client.post(
                url,
                headers={"Authorization": f"Bearer {api_key}"},
                json=payload,
            )
            total_http_seconds += time.perf_counter() - started
            response.raise_for_status()
            body = response.json()
            choices = body.get("choices")
            if not isinstance(choices, list) or len(choices) != 1:
                raise RuntimeError("provider response must contain exactly one choice")
            choice = choices[0]
            if choice.get("finish_reason") != "stop":
                raise RuntimeError(
                    f"provider response finished with {choice.get('finish_reason')!r}"
                )
            content = choice.get("message", {}).get("content")
            if not isinstance(content, str) or not content.strip():
                raise RuntimeError("provider response contains no assistant text")
            return (
                content,
                body.get("usage") or {},
                {
                    "provider_attempts": provider_attempt + 1,
                    "provider_request_number": request_number,
                    "http_seconds": total_http_seconds,
                    "response_id": body.get("id"),
                },
            )
        except (httpx.HTTPError, json.JSONDecodeError, RuntimeError) as error:
            if response is not None and response.status_code not in {
                408,
                429,
                500,
                502,
                503,
                504,
            }:
                raise RuntimeError(
                    f"non-retryable provider response {response.status_code}: "
                    f"{response.text[:2_000]}"
                ) from error
            errors.append(str(error))
            if provider_attempt == max_retries:
                raise RuntimeError(
                    f"provider request failed after {provider_attempt + 1} attempts: "
                    + " | ".join(errors)
                ) from error
            await asyncio.sleep(retry_delay(response, provider_attempt + 1))
    raise AssertionError("provider retry loop did not return")


def strip_json_fence(content: str) -> str:
    stripped = content.strip()
    if not stripped.startswith("```"):
        return stripped
    lines = stripped.splitlines()
    if len(lines) < 3 or lines[-1].strip() != "```":
        return stripped
    return "\n".join(lines[1:-1])


def validate_comparisons(
    content: str, batch: list[dict[str, Any]]
) -> list[dict[str, int]]:
    candidate = json.loads(strip_json_fence(content))
    if not isinstance(candidate, dict) or set(candidate) != {"comparisons"}:
        raise ValueError("top-level JSON must contain only comparisons")
    comparisons = candidate["comparisons"]
    if not isinstance(comparisons, list):
        raise TypeError("comparisons must be an array")
    expected = {quartet["quartet_id"]: quartet for quartet in batch}
    actual: dict[int, dict[str, int]] = {}
    for comparison in comparisons:
        if not isinstance(comparison, dict) or set(comparison) != {
            "quartet_id",
            "most_important",
            "least_important",
        }:
            raise ValueError(
                "every comparison must contain exactly the three required fields"
            )
        if not all(isinstance(value, int) for value in comparison.values()):
            raise ValueError("comparison identifiers must be integers")
        quartet_id = comparison["quartet_id"]
        if quartet_id in actual:
            raise ValueError(f"quartet {quartet_id} was returned more than once")
        quartet = expected.get(quartet_id)
        if quartet is None:
            raise ValueError(f"unknown quartet_id {quartet_id}")
        members = quartet["passage_ids"]
        most = comparison["most_important"]
        least = comparison["least_important"]
        if most not in members or least not in members:
            raise ValueError(
                f"quartet {quartet_id} selected most={most} and least={least}, "
                f"but its allowed passage_ids are {members}"
            )
        if most == least:
            raise ValueError(
                f"quartet {quartet_id} selected the same most and least passage"
            )
        actual[quartet_id] = comparison
    if set(actual) != set(expected):
        missing = sorted(set(expected) - set(actual))
        raise ValueError(f"response omitted quartet IDs {missing}")
    return [actual[quartet["quartet_id"]] for quartet in batch]


def batch_input(
    batch: list[dict[str, Any]],
    passages: list[dict[str, Any]],
    context: str,
) -> str:
    return json.dumps(
        {
            "lecture_context": context,
            "quartets": [
                {
                    "quartet_id": quartet["quartet_id"],
                    "passages": [
                        {
                            "passage_id": passage_id,
                            "text": passages[passage_id]["text"],
                        }
                        for passage_id in quartet["passage_ids"]
                    ],
                }
                for quartet in batch
            ],
        },
        ensure_ascii=False,
    )


async def compare_batch(
    batch_index: int,
    batch: list[dict[str, Any]],
    passages: list[dict[str, Any]],
    context: str,
    client: httpx.AsyncClient,
    provider: dict[str, Any],
    budget: RequestBudget,
    limiter: RequestRateLimiter,
    max_retries: int,
    max_repairs: int,
) -> dict[str, Any]:
    started = time.perf_counter()
    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": batch_input(batch, passages, context)},
    ]
    raw_responses = []
    provider_calls = 0
    provider_attempts = 0
    http_seconds = 0.0
    prompt_tokens = 0
    completion_tokens = 0
    token_counts_complete = True

    for repair in range(max_repairs + 1):
        content, usage, diagnostics = await provider_chat(
            client,
            provider["url"],
            provider["api_key"],
            provider["model"],
            provider["extra_body"],
            messages,
            budget,
            limiter,
            max_retries,
        )
        provider_calls = repair + 1
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
            comparisons = validate_comparisons(content, batch)
            return {
                "format_version": 1,
                "batch_index": batch_index,
                "quartets": batch,
                "comparisons": comparisons,
                "diagnostics": {
                    "elapsed_seconds": time.perf_counter() - started,
                    "http_seconds": http_seconds,
                    "provider_calls": provider_calls,
                    "provider_attempts": provider_attempts,
                    "json_repairs": repair,
                    "prompt_tokens": prompt_tokens if token_counts_complete else None,
                    "completion_tokens": completion_tokens
                    if token_counts_complete
                    else None,
                },
                "raw_responses": raw_responses,
            }
        except (json.JSONDecodeError, ValueError) as error:
            if repair == max_repairs:
                raise RuntimeError(
                    f"batch {batch_index} remained invalid after {repair} repairs: {error}"
                ) from error
            messages.extend(
                [
                    {"role": "assistant", "content": content},
                    {
                        "role": "user",
                        "content": (
                            f"上一份 JSON 无效：{error}。请修复它。必须完整回答原来的 "
                            "quartets，只返回符合指定形状的 JSON。"
                        ),
                    },
                ]
            )
    raise AssertionError("JSON repair loop did not return")


def load_checkpoint(path: Path, batch: list[dict[str, Any]]) -> dict[str, Any]:
    checkpoint = read_json(path)
    if checkpoint.get("quartets") != batch:
        raise ValueError(
            f"checkpoint {path} does not match the generated quartet batch"
        )
    validate_comparisons(
        json.dumps({"comparisons": checkpoint.get("comparisons")}), batch
    )
    return checkpoint


async def run_comparisons(
    comparison_batches: list[list[dict[str, Any]]],
    passages: list[dict[str, Any]],
    context: str,
    output_directory: Path,
    provider: dict[str, Any],
    arguments: argparse.Namespace,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    checkpoints: list[dict[str, Any] | None] = [None] * len(comparison_batches)
    pending = []
    for batch_index, batch in enumerate(comparison_batches):
        path = output_directory / f"batch-{batch_index:04}.json"
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
    completed = len(comparison_batches) - sum(item is None for item in checkpoints)
    started_at = datetime.now(UTC).isoformat()
    started = time.perf_counter()

    async with httpx.AsyncClient(
        timeout=httpx.Timeout(240.0, connect=30.0),
        limits=httpx.Limits(max_connections=arguments.concurrency),
    ) as client:

        async def run_one(batch_index: int, batch: list[dict[str, Any]]) -> None:
            nonlocal completed
            async with semaphore:
                checkpoint = await compare_batch(
                    batch_index,
                    batch,
                    passages,
                    context,
                    client,
                    provider,
                    budget,
                    limiter,
                    arguments.max_provider_retries,
                    arguments.max_json_repairs,
                )
                write_json_atomically(
                    output_directory / f"batch-{batch_index:04}.json", checkpoint
                )
                checkpoints[batch_index] = checkpoint
                async with progress_lock:
                    completed += 1
                    print(
                        f"[{completed:>3}/{len(comparison_batches)}] "
                        f"batch {batch_index:04} · "
                        f"{checkpoint['diagnostics']['elapsed_seconds']:.1f}s · "
                        f"{checkpoint['diagnostics']['json_repairs']} repairs",
                        flush=True,
                    )

        await asyncio.gather(*(run_one(index, batch) for index, batch in pending))

    invocation = {
        "started_at": started_at,
        "finished_at": datetime.now(UTC).isoformat(),
        "wall_seconds": time.perf_counter() - started,
        "provider_requests": budget.count,
        "new_batches": len(pending),
        "canary_only": arguments.canary_only,
    }
    completed_checkpoints = [
        checkpoint for checkpoint in checkpoints if checkpoint is not None
    ]
    return completed_checkpoints, invocation


def average_ranks(values: list[float]) -> list[float]:
    ranked = sorted(enumerate(values), key=lambda item: item[1])
    ranks = [0.0] * len(values)
    start = 0
    while start < len(ranked):
        end = start + 1
        while end < len(ranked) and ranked[end][1] == ranked[start][1]:
            end += 1
        rank = (start + end - 1) / 2
        for original_index, _ in ranked[start:end]:
            ranks[original_index] = rank
        start = end
    return ranks


def pearson(left: list[float], right: list[float]) -> float:
    left_mean = statistics.fmean(left)
    right_mean = statistics.fmean(right)
    numerator = sum(
        (left_value - left_mean) * (right_value - right_mean)
        for left_value, right_value in zip(left, right, strict=True)
    )
    left_variance = sum((value - left_mean) ** 2 for value in left)
    right_variance = sum((value - right_mean) ** 2 for value in right)
    denominator = math.sqrt(left_variance * right_variance)
    return numerator / denominator if denominator else 0.0


def spearman(left: list[float], right: list[float]) -> float:
    return pearson(average_ranks(left), average_ranks(right))


def add_observation(
    counts: list[dict[str, int]],
    quartet: dict[str, Any],
    comparison: dict[str, int],
) -> None:
    for passage_id in quartet["passage_ids"]:
        counts[passage_id]["appearances"] += 1
    counts[comparison["most_important"]]["most"] += 1
    counts[comparison["least_important"]]["least"] += 1


def scores_from_counts(counts: list[dict[str, int]]) -> list[float]:
    return [
        (count["most"] - count["least"]) / count["appearances"]
        if count["appearances"]
        else 0.0
        for count in counts
    ]


def percentiles(values: list[float]) -> list[float]:
    ranks = average_ranks(values)
    denominator = max(1, len(values) - 1)
    return [rank * 100 / denominator for rank in ranks]


def aggregate(
    passages: list[dict[str, Any]],
    quartets: list[dict[str, Any]],
    checkpoints: list[dict[str, Any]],
    rounds: int,
) -> dict[str, Any]:
    passage_count = len(passages)
    counts = [{"appearances": 0, "most": 0, "least": 0} for _ in passages]
    first_half = [{"appearances": 0, "most": 0, "least": 0} for _ in passages]
    second_half = [{"appearances": 0, "most": 0, "least": 0} for _ in passages]
    position_most = Counter()
    position_least = Counter()
    by_quartet = {quartet["quartet_id"]: quartet for quartet in quartets}

    for checkpoint in checkpoints:
        for comparison in checkpoint["comparisons"]:
            quartet = by_quartet[comparison["quartet_id"]]
            add_observation(counts, quartet, comparison)
            half = first_half if quartet["round"] < rounds // 2 else second_half
            add_observation(half, quartet, comparison)
            position_most[
                quartet["passage_ids"].index(comparison["most_important"])
            ] += 1
            position_least[
                quartet["passage_ids"].index(comparison["least_important"])
            ] += 1

    scores = scores_from_counts(counts)
    relative_percentiles = percentiles(scores)
    first_scores = scores_from_counts(first_half)
    second_scores = scores_from_counts(second_half)
    has_reference_importance = all(
        isinstance(passage.get("importance"), int) for passage in passages
    )
    old_importance = (
        [float(passage["importance"]) for passage in passages]
        if has_reference_importance
        else None
    )
    lengths = [float(len(passage["text"])) for passage in passages]
    results = [
        {
            "passage_id": passage_id,
            "text": passage["text"],
            "old_importance": passage.get("importance"),
            **counts[passage_id],
            "best_worst_score": scores[passage_id],
            "importance_percentile": relative_percentiles[passage_id],
        }
        for passage_id, passage in enumerate(passages)
    ]
    old_groups = defaultdict(list)
    if has_reference_importance:
        for result in results:
            old_groups[result["old_importance"]].append(result["importance_percentile"])
    quartet_count = len(quartets)
    return {
        "passages": results,
        "metrics": {
            "passage_count": passage_count,
            "quartet_count": quartet_count,
            "unique_best_worst_scores": len(set(scores)),
            "reference_importance_available": has_reference_importance,
            "old_importance_distribution": (
                dict(sorted(Counter(old_importance).items()))
                if old_importance is not None
                else None
            ),
            "old_importance_vs_best_worst_spearman": (
                spearman(old_importance, scores) if old_importance is not None else None
            ),
            "old_importance_vs_length_spearman": (
                spearman(old_importance, lengths)
                if old_importance is not None
                else None
            ),
            "best_worst_vs_length_spearman": spearman(scores, lengths),
            "half_run_stability_spearman": spearman(first_scores, second_scores),
            "most_choice_position_percent": [
                position_most[position] * 100 / quartet_count for position in range(4)
            ],
            "least_choice_position_percent": [
                position_least[position] * 100 / quartet_count for position in range(4)
            ],
            "old_level_percentiles": {
                str(level): {
                    "count": len(values),
                    "mean": statistics.fmean(values),
                    "median": statistics.median(values),
                    "minimum": min(values),
                    "maximum": max(values),
                }
                for level, values in sorted(old_groups.items())
            },
        },
    }


def total_or_none(checkpoints: list[dict[str, Any]], field: str) -> int | None:
    values = [checkpoint["diagnostics"].get(field) for checkpoint in checkpoints]
    return sum(values) if all(isinstance(value, int) for value in values) else None


def collapse_text(text: str, limit: int = 150) -> str:
    collapsed = " ".join(text.split())
    return collapsed if len(collapsed) <= limit else collapsed[: limit - 1] + "…"


def markdown_cell(text: str) -> str:
    return collapse_text(text).replace("|", "\\|")


def format_duration(seconds: float) -> str:
    minutes, remainder = divmod(seconds, 60)
    hours, minutes = divmod(int(minutes), 60)
    return f"{hours:02}:{minutes:02}:{remainder:04.1f}"


def markdown_report(
    manifest: dict[str, Any],
    aggregate_result: dict[str, Any],
    checkpoints: list[dict[str, Any]],
    invocation: dict[str, Any],
) -> str:
    metrics = aggregate_result["metrics"]
    diagnostics = [checkpoint["diagnostics"] for checkpoint in checkpoints]
    ranked = sorted(
        aggregate_result["passages"],
        key=lambda passage: (-passage["best_worst_score"], passage["passage_id"]),
    )
    lines = [
        "# Best--worst lecture-importance experiment",
        "",
        "## Run",
        "",
        f"- Model: `{manifest['model']}`",
        f"- Passages: {metrics['passage_count']}",
        f"- Quartets: {metrics['quartet_count']}",
        f"- Batches: {len(checkpoints)}",
        f"- Final invocation wall time: {format_duration(invocation['wall_seconds'])} ({invocation['wall_seconds']:.3f} seconds)",
        f"- Provider requests in final invocation: {invocation['provider_requests']}",
        f"- Provider attempts in accepted checkpoints: {sum(item['provider_attempts'] for item in diagnostics)}",
        f"- JSON repairs: {sum(item['json_repairs'] for item in diagnostics)}",
        f"- Summed HTTP time for accepted checkpoints: {format_duration(sum(item['http_seconds'] for item in diagnostics))}",
        f"- Prompt tokens: {total_or_none(checkpoints, 'prompt_tokens')}",
        f"- Completion tokens: {total_or_none(checkpoints, 'completion_tokens')}",
        "",
        "## Diagnostics",
        "",
        f"- Unique best--worst scores: {metrics['unique_best_worst_scores']}",
        f"- Best--worst rank vs passage length (Spearman): {metrics['best_worst_vs_length_spearman']:.3f}",
        f"- First-half vs second-half stability (Spearman): {metrics['half_run_stability_spearman']:.3f}",
    ]
    if metrics["reference_importance_available"]:
        lines.extend(
            [
                f"- Old importance vs best--worst rank (Spearman): {metrics['old_importance_vs_best_worst_spearman']:.3f}",
                f"- Old importance vs passage length (Spearman): {metrics['old_importance_vs_length_spearman']:.3f}",
            ]
        )
    lines.extend(
        [
            "",
            "Choice position percentages (an unbiased result should be near 25% per column):",
            "",
            "| Choice | Position 1 | Position 2 | Position 3 | Position 4 |",
            "| --- | ---: | ---: | ---: | ---: |",
            "| Most | "
            + " | ".join(
                f"{value:.1f}%" for value in metrics["most_choice_position_percent"]
            )
            + " |",
            "| Least | "
            + " | ".join(
                f"{value:.1f}%" for value in metrics["least_choice_position_percent"]
            )
            + " |",
        ]
    )
    if metrics["reference_importance_available"]:
        lines.extend(
            [
                "",
                "## How old levels map to comparative percentiles",
                "",
                "| Old importance | Count | Mean percentile | Median | Range |",
                "| ---: | ---: | ---: | ---: | ---: |",
            ]
        )
        for level, values in metrics["old_level_percentiles"].items():
            lines.append(
                f"| {level} | {values['count']} | {values['mean']:.1f} | "
                f"{values['median']:.1f} | {values['minimum']:.1f}–{values['maximum']:.1f} |"
            )
    old_column = " Old |" if metrics["reference_importance_available"] else ""
    old_alignment = " ---: |" if metrics["reference_importance_available"] else ""
    lines.extend(
        [
            "",
            "## Highest comparative importance",
            "",
            f"| Rank | Passage |{old_column} BWS | Percentile | Text |",
            f"| ---: | ---: |{old_alignment} ---: | ---: | --- |",
        ]
    )
    for rank, passage in enumerate(ranked[:20], 1):
        old_value = (
            f" {passage['old_importance']} |"
            if metrics["reference_importance_available"]
            else ""
        )
        lines.append(
            f"| {rank} | {passage['passage_id']} |{old_value} "
            f"{passage['best_worst_score']:.3f} | {passage['importance_percentile']:.1f} | "
            f"{markdown_cell(passage['text'])} |"
        )
    lines.extend(
        [
            "",
            "## Lowest comparative importance",
            "",
            f"| Rank | Passage |{old_column} BWS | Percentile | Text |",
            f"| ---: | ---: |{old_alignment} ---: | ---: | --- |",
        ]
    )
    for rank, passage in enumerate(reversed(ranked[-20:]), 1):
        old_value = (
            f" {passage['old_importance']} |"
            if metrics["reference_importance_available"]
            else ""
        )
        lines.append(
            f"| {rank} | {passage['passage_id']} |{old_value} "
            f"{passage['best_worst_score']:.3f} | {passage['importance_percentile']:.1f} | "
            f"{markdown_cell(passage['text'])} |"
        )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    arguments = parse_arguments()
    analysis = read_json(arguments.analysis)
    slides = read_json(arguments.slides)
    passages = validate_inputs(analysis, slides)
    quartets = build_quartets(len(passages), arguments.rounds, arguments.seed)
    comparison_batches = batches(quartets, arguments.quartets_per_request)
    base_url = required_environment("BEYOND_SLIDES_API_BASE_URL").rstrip("/")
    model = required_environment("BEYOND_SLIDES_MODEL")
    provider = {
        "url": f"{base_url}/chat/completions",
        "api_key": required_environment("BEYOND_SLIDES_API_KEY"),
        "model": model,
        "extra_body": optional_extra_body(),
    }
    manifest = {
        "format_version": 1,
        "prompt_version": PROMPT_VERSION,
        "prompt_sha256": hashlib.sha256(SYSTEM_PROMPT.encode()).hexdigest(),
        "analysis_sha256": sha256(arguments.analysis),
        "slides_sha256": sha256(arguments.slides),
        "api_base_url": base_url,
        "model": model,
        "extra_body": provider["extra_body"],
        "passage_count": len(passages),
        "rounds": arguments.rounds,
        "quartet_count": len(quartets),
        "quartets_per_request": arguments.quartets_per_request,
        "batch_count": len(comparison_batches),
        "seed": arguments.seed,
        "concurrency": arguments.concurrency,
        "minimum_request_interval_seconds": arguments.minimum_request_interval,
        "max_provider_retries": arguments.max_provider_retries,
        "max_json_repairs": arguments.max_json_repairs,
        "max_provider_requests": arguments.max_provider_requests,
    }
    arguments.output_directory.mkdir(parents=True, exist_ok=True)
    manifest_path = arguments.output_directory / "manifest.json"
    if manifest_path.exists() and read_json(manifest_path) != manifest:
        raise ValueError(
            f"existing manifest {manifest_path} does not match this experiment"
        )
    write_json_atomically(manifest_path, manifest)
    write_json_atomically(arguments.output_directory / "quartets.json", quartets)

    checkpoints, invocation = asyncio.run(
        run_comparisons(
            comparison_batches,
            passages,
            lecture_context(slides),
            arguments.output_directory,
            provider,
            arguments,
        )
    )
    write_json_atomically(
        arguments.output_directory / "last-invocation.json", invocation
    )
    if len(checkpoints) != len(comparison_batches):
        print(
            f"Canary complete: {len(checkpoints)}/{len(comparison_batches)} batches checkpointed.",
            file=sys.stderr,
        )
        return 0

    aggregate_result = aggregate(passages, quartets, checkpoints, arguments.rounds)
    summary = {
        "manifest": manifest,
        "timing": invocation,
        "diagnostics": aggregate_result["metrics"],
    }
    write_json_atomically(arguments.output_directory / "results.json", aggregate_result)
    write_json_atomically(arguments.output_directory / "summary.json", summary)
    write_text_atomically(
        arguments.output_directory / "report.md",
        markdown_report(manifest, aggregate_result, checkpoints, invocation),
    )
    print(f"Wrote {arguments.output_directory / 'report.md'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, TypeError, ValueError, RuntimeError) as error:
        print(f"importance comparison: {error}", file=sys.stderr)
        raise SystemExit(1) from error
