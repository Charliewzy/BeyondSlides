# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""PROTOTYPE: classify restored-text boundaries, then partition them with DP."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import statistics
import sys
import time
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
    write_text_atomically,
)
from prototype_semantic_segmentation import load_source_windows

PROMPT_VERSION = 2
PRIMARY_ENDINGS = frozenset("。！？!?；;：:\n")
WEAK_ENDINGS = frozenset("，,")
ALL_ENDINGS = PRIMARY_ENDINGS | WEAK_ENDINGS
STRENGTHS = ("continue", "possible_break", "preferred_break", "required_break")

SYSTEM_PROMPT = """\
你负责判断大学课堂讲稿中的候选语义边界强度。

输入已由恢复阶段添加标点，并按标点分成不可再分的原文 atoms。每个 boundary 位于
after_atom 之后。你只判断边界，不得改写文本，不得评价 importance、novelty 或内容价值。

对每个 owned_boundary_after_atom_id 必须且只能返回一项：
- continue：两侧在语法或理解上不可分离，如设问与紧随回答、因果句未完成、主张与不可缺少的
  紧随解释。在此分段会产生半句、孤立设问或无法独立理解的片段。continue 应该稀少；
  仅仅因为两侧属于同一教学动作或同一主题，不足以选 continue。
- possible_break：两侧都是可以独立理解的完整表达，但仍服务于同一个教学动作。当该动作过长时，
  这里是可接受的次优分段处。
- preferred_break：后文开始一个可独立复习的教学动作，如新主张、新定义、新例子、对比、限制、
  推论、实践步骤或新子问题。仍属同一大主题不妨碍成为 preferred_break。
- required_break：显式换章节、换主题、换幻灯片单元，或从一个已完整教学单元明确转入另一个单元。

不要因为两侧都在讲“泛型”等同一上位主题就选 continue。判断的是教学动作是否已经切换。
下游组装绝不会在 continue 处分段。因此，请检查每个 window 中的连续文本：除非确实存在一个超过
450 字且不可分离的完整教学单元，不应出现连续 300–400 字之间全部是 continue 的情况。
应将其中损害最小的完整表达边界标为 possible_break。
context atoms 只用来理解边界；仍必须精确回答所有 owned boundary ID。

只返回 JSON，不要 Markdown 或解释。完整形状：
{
  "windows": [
    {
      "window_index": 0,
      "boundaries": [
        {"after_atom": 12, "strength": "preferred_break"}
      ]
    }
  ]
}
"""


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("analysis_run_directory", type=Path)
    parser.add_argument("baseline_segmentation", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--owned-boundaries-per-window", type=int, default=48)
    parser.add_argument("--context-atoms", type=int, default=8)
    parser.add_argument("--windows-per-request", type=int, default=2)
    parser.add_argument("--long-atom-characters", type=int, default=180)
    parser.add_argument("--preferred-min-characters", type=int, default=80)
    parser.add_argument("--preferred-max-characters", type=int, default=280)
    parser.add_argument("--hard-max-characters", type=int, default=450)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--minimum-request-interval", type=float, default=5.0)
    parser.add_argument("--max-provider-retries", type=int, default=5)
    parser.add_argument("--max-json-repairs", type=int, default=2)
    parser.add_argument("--max-provider-requests", type=int, default=256)
    parser.add_argument("--canary-only", action="store_true")
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument(
        "--heuristic-only",
        action="store_true",
        help="exercise the local DP without contacting the provider",
    )
    return parser.parse_args()


def terminal_kind(text: str) -> str:
    for character in reversed(text):
        if character in "\n":
            return "line"
        if character in "。！？!?":
            return "sentence"
        if character in "；;":
            return "semicolon"
        if character in "：:":
            return "colon"
        if character in "，,":
            return "comma"
    return "other"


def split_at_endings(text: str, endings: frozenset[str]) -> list[str]:
    pieces = []
    start = 0
    for index, character in enumerate(text):
        if character not in endings:
            continue
        end = index + 1
        if end > start:
            pieces.append(text[start:end])
        start = end
    if start < len(text):
        pieces.append(text[start:])
    return pieces


def atomize(text: str, long_atom_characters: int) -> list[dict[str, Any]]:
    if long_atom_characters <= 0:
        raise ValueError("long atom characters must be greater than zero")
    pieces = []
    for primary_piece in split_at_endings(text, PRIMARY_ENDINGS):
        if len(primary_piece) > long_atom_characters:
            pieces.extend(split_at_endings(primary_piece, WEAK_ENDINGS))
        else:
            pieces.append(primary_piece)
    atoms = []
    next_character = 0
    for piece in pieces:
        if not piece:
            continue
        end = next_character + len(piece)
        atoms.append(
            {
                "atom_id": len(atoms),
                "text": piece,
                "start": next_character,
                "end": end,
                "terminal_kind": terminal_kind(piece),
            }
        )
        next_character = end
    if not atoms or next_character != len(text):
        raise AssertionError("atoms do not partition restored text")
    return atoms


def flatten_source_windows(
    source_windows: list[dict[str, Any]],
) -> tuple[str, list[dict[str, Any]]]:
    text_parts = []
    spans = []
    next_character = 0
    for window in source_windows:
        text_parts.append(window["text"])
        for span in window["source_spans"]:
            spans.append(
                {
                    **span,
                    "start": span["start"] + next_character,
                    "end": span["end"] + next_character,
                }
            )
        next_character += len(window["text"])
    text = "".join(text_parts)
    if not text or spans[-1]["end"] != len(text):
        raise AssertionError("flattened provenance does not cover restored text")
    return text, spans


def build_boundary_windows(
    atoms: list[dict[str, Any]], owned_count: int, context_atoms: int
) -> list[dict[str, Any]]:
    if owned_count <= 0:
        raise ValueError("owned boundaries per window must be greater than zero")
    if context_atoms < 0:
        raise ValueError("context atoms cannot be negative")
    gap_count = len(atoms) - 1
    windows = []
    for owned_start in range(0, gap_count, owned_count):
        owned_end = min(gap_count, owned_start + owned_count)
        visible_start = max(0, owned_start - context_atoms)
        visible_end = min(len(atoms), owned_end + 1 + context_atoms)
        windows.append(
            {
                "window_index": len(windows),
                "owned_boundary_after_atom_ids": list(range(owned_start, owned_end)),
                "atoms": [
                    {"atom_id": atom["atom_id"], "text": atom["text"]}
                    for atom in atoms[visible_start:visible_end]
                ],
            }
        )
    return windows


def batch_input(batch: list[dict[str, Any]]) -> str:
    return json.dumps({"windows": batch}, ensure_ascii=False)


def validate_response(
    content: str, batch: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    candidate = json.loads(strip_json_fence(content))
    if not isinstance(candidate, dict) or set(candidate) != {"windows"}:
        raise ValueError("top-level JSON must contain only windows")
    expected = {window["window_index"]: window for window in batch}
    actual = {}
    for proposed_window in candidate["windows"]:
        if not isinstance(proposed_window, dict) or set(proposed_window) != {
            "window_index",
            "boundaries",
        }:
            raise ValueError("every window must contain window_index and boundaries")
        window_index = proposed_window["window_index"]
        source = expected.get(window_index)
        if source is None or window_index in actual:
            raise ValueError(f"unknown or duplicate window_index {window_index!r}")
        expected_ids = set(source["owned_boundary_after_atom_ids"])
        boundaries = {}
        if not isinstance(proposed_window["boundaries"], list):
            raise TypeError("boundaries must be an array")
        for boundary in proposed_window["boundaries"]:
            if not isinstance(boundary, dict) or set(boundary) != {
                "after_atom",
                "strength",
            }:
                raise ValueError("every boundary must contain after_atom and strength")
            after_atom = boundary["after_atom"]
            strength = boundary["strength"]
            if not isinstance(after_atom, int) or after_atom not in expected_ids:
                raise ValueError(
                    f"window {window_index} returned boundary {after_atom!r}"
                )
            if after_atom in boundaries:
                raise ValueError(
                    f"window {window_index} repeated boundary {after_atom}"
                )
            if strength not in STRENGTHS:
                raise ValueError(f"boundary {after_atom} has strength {strength!r}")
            boundaries[after_atom] = strength
        if set(boundaries) != expected_ids:
            raise ValueError(
                f"window {window_index} omitted boundaries "
                f"{sorted(expected_ids - set(boundaries))}"
            )
        actual[window_index] = {
            "window_index": window_index,
            "boundaries": [
                {"after_atom": boundary_id, "strength": boundaries[boundary_id]}
                for boundary_id in source["owned_boundary_after_atom_ids"]
            ],
        }
    if set(actual) != set(expected):
        raise ValueError(
            f"response omitted windows {sorted(set(expected) - set(actual))}"
        )
    return [actual[window["window_index"]] for window in batch]


async def classify_batch(
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
    provider_attempts = 0
    http_seconds = 0.0
    prompt_tokens = 0
    completion_tokens = 0
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
                "source_windows": batch,
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
                            f"上一份 JSON 无效：{error}。请修复它，完整回答原来的 "
                            "windows，只返回指定 JSON。"
                        ),
                    },
                ]
            )
    raise AssertionError("repair loop did not return")


def load_checkpoint(path: Path, batch: list[dict[str, Any]]) -> dict[str, Any]:
    checkpoint = read_json(path)
    if checkpoint.get("source_windows") != batch:
        raise ValueError(
            f"checkpoint {path} does not match its source boundary windows"
        )
    return checkpoint


async def run_batches(
    source_batches: list[list[dict[str, Any]]],
    output_directory: Path,
    provider: dict[str, Any],
    arguments: argparse.Namespace,
) -> tuple[list[dict[str, Any]], float]:
    checkpoints: list[dict[str, Any] | None] = [None] * len(source_batches)
    pending = []
    for batch_index, batch in enumerate(source_batches):
        path = output_directory / f"boundary-batch-{batch_index:04}.json"
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
    started = time.perf_counter()

    import httpx

    async with httpx.AsyncClient(
        timeout=httpx.Timeout(240.0, connect=30.0),
        limits=httpx.Limits(max_connections=arguments.concurrency),
    ) as client:

        async def run_one(batch_index: int, batch: list[dict[str, Any]]) -> None:
            nonlocal completed
            async with semaphore:
                checkpoint = await classify_batch(
                    batch_index,
                    batch,
                    client,
                    provider,
                    budget,
                    limiter,
                    arguments,
                )
                write_json_atomically(
                    output_directory / f"boundary-batch-{batch_index:04}.json",
                    checkpoint,
                )
                checkpoints[batch_index] = checkpoint
                async with progress_lock:
                    completed += 1
                    print(
                        f"[{completed:>3}/{len(source_batches)}] "
                        f"boundary batch {batch_index:04} · "
                        f"{checkpoint['diagnostics']['elapsed_seconds']:.1f}s · "
                        f"{checkpoint['diagnostics']['json_repairs']} repairs",
                        flush=True,
                    )

        await asyncio.gather(*(run_one(index, batch) for index, batch in pending))
    return (
        [checkpoint for checkpoint in checkpoints if checkpoint is not None],
        time.perf_counter() - started,
    )


def heuristic_classifications(atoms: list[dict[str, Any]]) -> list[str]:
    required_prefixes = ("好，那么接下来", "好，我们接下来", "下面我们", "最后我们")
    preferred_prefixes = (
        "那么",
        "首先",
        "其次",
        "另外",
        "还有一种",
        "举个例子",
        "比如说",
        "所以大家可以看到",
    )
    strengths = []
    for atom, following in pairwise(atoms):
        following_text = following["text"].lstrip()
        if following_text.startswith(required_prefixes):
            strengths.append("required_break")
        elif following_text.startswith(preferred_prefixes):
            strengths.append("preferred_break")
        elif atom["terminal_kind"] in {"sentence", "line"}:
            strengths.append("possible_break")
        else:
            strengths.append("continue")
    return strengths


def aggregate_classifications(
    checkpoints: list[dict[str, Any]], gap_count: int
) -> list[str]:
    classifications: list[str | None] = [None] * gap_count
    for checkpoint in checkpoints:
        for window in checkpoint["windows"]:
            for boundary in window["boundaries"]:
                after_atom = boundary["after_atom"]
                if classifications[after_atom] is not None:
                    raise ValueError(f"boundary {after_atom} was classified twice")
                classifications[after_atom] = boundary["strength"]
    missing = [
        index for index, strength in enumerate(classifications) if strength is None
    ]
    if missing:
        raise ValueError(f"boundary classifications are incomplete: {missing[:8]}")
    return [strength for strength in classifications if strength is not None]


def passage_length_score(length: int, preferred_min: int, preferred_max: int) -> float:
    if length < preferred_min:
        return -(preferred_min - length) / 8
    if length > preferred_max:
        return -(length - preferred_max) / 12
    return 0.0


def partition_with_dp(
    atoms: list[dict[str, Any]],
    strengths: list[str],
    preferred_min: int,
    preferred_max: int,
    hard_max: int,
) -> list[tuple[int, int]]:
    if not 0 < preferred_min <= preferred_max < hard_max:
        raise ValueError("expected 0 < preferred min <= preferred max < hard max")
    if len(strengths) != len(atoms) - 1:
        raise ValueError("one strength is required between every adjacent atom")
    rewards = {
        "continue": -14.0,
        "possible_break": -2.0,
        "preferred_break": 4.0,
        "required_break": 16.0,
    }
    prefix_lengths = [0]
    for atom in atoms:
        prefix_lengths.append(prefix_lengths[-1] + len(atom["text"]))
    oversized_atoms = [atom for atom in atoms if len(atom["text"]) > hard_max]
    if oversized_atoms:
        atom = oversized_atoms[0]
        raise ValueError(
            f"atom {atom['atom_id']} contains {len(atom['text'])} characters, "
            f"exceeding the hard passage maximum {hard_max}"
        )
    best = [float("-inf")] * (len(atoms) + 1)
    previous: list[int | None] = [None] * (len(atoms) + 1)
    best[0] = 0.0
    for end in range(1, len(atoms) + 1):
        candidates = []
        for start in range(end - 1, -1, -1):
            length = prefix_lengths[end] - prefix_lengths[start]
            if length > hard_max:
                break
            if best[start] == float("-inf"):
                continue
            if end < len(atoms) and strengths[end - 1] == "continue":
                continue
            boundary_reward = rewards[strengths[end - 1]] if end < len(atoms) else 0
            score = (
                best[start]
                + passage_length_score(length, preferred_min, preferred_max)
                + boundary_reward
            )
            candidates.append((score, start))
        if candidates:
            best[end], previous[end] = max(candidates)

    ranges = []
    end = len(atoms)
    if previous[end] is None:
        raise ValueError(
            f"no complete partition fits within the hard maximum {hard_max}; "
            "at least one run between legal boundaries is too long"
        )
    while end:
        start = previous[end]
        if start is None:
            raise AssertionError("DP result has no predecessor")
        ranges.append((start, end))
        end = start
    ranges.reverse()
    return ranges


def provenance(spans: list[dict[str, Any]], start: int, end: int) -> dict[str, Any]:
    overlapping = [
        span for span in spans if start < span["end"] and end > span["start"]
    ]
    if not overlapping:
        raise ValueError(f"passage {start}..{end} has no source provenance")
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


def assemble_passages(
    atoms: list[dict[str, Any]],
    ranges: list[tuple[int, int]],
    strengths: list[str],
    spans: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    passages = []
    for passage_id, (start_atom, end_atom) in enumerate(ranges):
        start = atoms[start_atom]["start"]
        end = atoms[end_atom - 1]["end"]
        text = "".join(atom["text"] for atom in atoms[start_atom:end_atom])
        if len(text) != end - start:
            raise AssertionError("passage text does not match its character range")
        passages.append(
            {
                "passage_id": passage_id,
                "text": text,
                "start_atom": start_atom,
                "end_atom": end_atom - 1,
                "end_boundary_strength": (
                    strengths[end_atom - 1] if end_atom < len(atoms) else "lecture_end"
                ),
                **provenance(spans, start, end),
            }
        )
    return passages


def stats(passages: list[dict[str, Any]]) -> dict[str, Any]:
    lengths = [len(passage["text"]) for passage in passages]
    return {
        "passage_count": len(passages),
        "minimum_characters": min(lengths),
        "median_characters": statistics.median(lengths),
        "mean_characters": statistics.fmean(lengths),
        "maximum_characters": max(lengths),
        "over_350_characters": sum(length > 350 for length in lengths),
        "over_450_characters": sum(length > 450 for length in lengths),
    }


def baseline_with_ranges(baseline: dict[str, Any]) -> list[dict[str, Any]]:
    passages = []
    next_character = 0
    for passage in baseline["passages"]:
        end = next_character + len(passage["text"])
        passages.append(
            {
                "passage_id": passage["passage_id"],
                "text": passage["text"],
                "start": next_character,
                "end": end,
            }
        )
        next_character = end
    return passages


def render_prototype(
    path: Path,
    atoms: list[dict[str, Any]],
    strengths: list[str],
    passages: list[dict[str, Any]],
    baseline: list[dict[str, Any]],
    configuration: dict[str, int],
) -> None:
    payload = json.dumps(
        {
            "atoms": atoms,
            "strengths": strengths,
            "initialPassages": passages,
            "baseline": baseline,
            "configuration": configuration,
        },
        ensure_ascii=False,
    ).replace("</", "<\\/")
    document = f"""<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Boundary + DP segmentation prototype</title>
<style>
  :root {{ color-scheme: light; font-family: Inter, "Noto Sans SC", sans-serif; }}
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; background: #f5f4fa; color: #201b2c; }}
  header {{ position: sticky; top: 0; z-index: 2; padding: 18px 24px; background: #fffdfdcc;
    backdrop-filter: blur(12px); border-bottom: 1px solid #ded9e8; }}
  h1 {{ margin: 0 0 6px; font-size: 20px; }}
  header p {{ margin: 0; color: #625a70; }}
  .controls {{ display: flex; flex-wrap: wrap; gap: 8px; margin-top: 14px; align-items: center; }}
  button {{ border: 1px solid #c9c0da; background: white; border-radius: 999px; padding: 8px 13px;
    cursor: pointer; color: inherit; }}
  button.active {{ background: #5033a3; color: white; border-color: #5033a3; }}
  .stats {{ display: flex; gap: 16px; flex-wrap: wrap; margin-top: 12px; font-size: 13px; }}
  .stats strong {{ font-size: 18px; display: block; color: #5033a3; }}
  main {{ max-width: 1120px; margin: 24px auto; padding: 0 24px 80px; }}
  .question {{ background: white; border: 1px solid #ded9e8; border-radius: 14px; padding: 16px 18px;
    margin-bottom: 18px; line-height: 1.6; }}
  .legend {{ display: flex; gap: 12px; flex-wrap: wrap; color: #625a70; font-size: 12px; margin-bottom: 12px; }}
  .passage {{ display: inline; font-family: "Noto Serif SC", serif; font-size: 21px; line-height: 2.05;
    padding: 3px 1px; border-radius: 5px; box-decoration-break: clone; -webkit-box-decoration-break: clone; }}
  .passage:nth-child(odd) {{ background: #eee8fb; }}
  .passage:nth-child(even) {{ background: #e4eefc; }}
  .passage:hover, .passage.focus {{ background: #ffdca8; outline: 2px solid #e48727; }}
  .boundary {{ display: inline-block; width: 3px; height: 1.25em; margin: 0 3px -0.2em; background: #5033a3; }}
  .meta {{ display: none; }}
  dialog {{ border: 0; border-radius: 14px; box-shadow: 0 18px 70px #251d3a55; max-width: 620px; }}
  dialog::backdrop {{ background: #21172c55; }}
  .walkthrough {{ margin-left: auto; }}
  @media (max-width: 760px) {{ .walkthrough {{ margin-left: 0; }} .passage {{ font-size: 17px; }} }}
</style>
</head>
<body>
<header>
  <h1>Boundary-strength + dynamic-programming segmentation</h1>
  <p>Question: do local semantic boundary judgments plus global length constraints create more coherent lecture passages than free-form partitioning?</p>
  <div class="controls">
    <button data-preset="fine">Finer</button>
    <button class="active" data-preset="recommended">Recommended</button>
    <button data-preset="loose">Looser</button>
    <button data-mode="dp" class="active">DP passages</button>
    <button data-mode="baseline">Previous prototype</button>
    <button class="walkthrough" data-jump="通过继承和多态">Inspect OOP → generics</button>
    <button data-jump="longest">Inspect longest</button>
  </div>
  <div class="stats" id="stats"></div>
</header>
<main>
  <section class="question">The coloured runs are passages, not score highlights. Click a passage to inspect its character count, atom range, and selected boundary strength. Presets rerun the pure DP locally; the model classifications remain fixed.</section>
  <div class="legend"><span>Lavender / blue: alternating passages</span><span>Violet bar: selected semantic boundary</span><span>Amber: walkthrough focus</span></div>
  <article id="transcript"></article>
</main>
<dialog id="details"><div id="details-content"></div><form method="dialog"><button>Close</button></form></dialog>
<script id="payload" type="application/json">{payload}</script>
<script>
const data = JSON.parse(document.getElementById('payload').textContent);
const rewards = {{continue: -14, possible_break: -2, preferred_break: 4, required_break: 16}};
const presets = {{
  fine: {{min: 55, preferredMax: 180, hardMax: 280}},
  recommended: {{min: 80, preferredMax: 280, hardMax: 450}},
  loose: {{min: 110, preferredMax: 340, hardMax: 520}}
}};
let mode = 'dp';
let current = presets.recommended;

function lengthScore(length, settings) {{
  if (length < settings.min) return -(settings.min - length) / 8;
  if (length > settings.preferredMax) return -(length - settings.preferredMax) / 12;
  return 0;
}}

function partition(atoms, strengths, settings) {{
  const prefix = [0];
  for (const atom of atoms) prefix.push(prefix.at(-1) + atom.text.length);
  const best = Array(atoms.length + 1).fill(-Infinity);
  const previous = Array(atoms.length + 1).fill(null);
  best[0] = 0;
  for (let end = 1; end <= atoms.length; end++) {{
    for (let start = end - 1; start >= 0; start--) {{
      const length = prefix[end] - prefix[start];
      if (length > settings.hardMax) break;
      if (end < atoms.length && strengths[end - 1] === 'continue') continue;
      const reward = end < atoms.length ? rewards[strengths[end - 1]] : 0;
      const score = best[start] + lengthScore(length, settings) + reward;
      if (score > best[end]) {{ best[end] = score; previous[end] = start; }}
    }}
  }}
  const result = [];
  for (let end = atoms.length; end > 0;) {{
    const start = previous[end];
    if (start === null) throw new Error(`No partition reaches atom ${{end}}`);
    result.push({{startAtom: start, endAtom: end - 1,
      text: atoms.slice(start, end).map(atom => atom.text).join(''),
      strength: end < atoms.length ? strengths[end - 1] : 'lecture_end'}});
    end = start;
  }}
  return result.reverse();
}}

function activePassages() {{
  if (mode === 'baseline') return data.baseline.map(item => ({{
    text: item.text, startAtom: null, endAtom: null, strength: 'unknown'
  }}));
  return partition(data.atoms, data.strengths, current);
}}

function render() {{
  const passages = activePassages();
  const lengths = passages.map(item => item.text.length).sort((a, b) => a - b);
  const mean = lengths.reduce((sum, value) => sum + value, 0) / lengths.length;
  document.getElementById('stats').innerHTML = `
    <span><strong>${{passages.length}}</strong>passages</span>
    <span><strong>${{lengths[Math.floor(lengths.length / 2)]}}</strong>median chars</span>
    <span><strong>${{mean.toFixed(1)}}</strong>mean chars</span>
    <span><strong>${{lengths.at(-1)}}</strong>maximum chars</span>`;
  const transcript = document.getElementById('transcript');
  transcript.replaceChildren();
  passages.forEach((passage, index) => {{
    const span = document.createElement('span');
    span.className = 'passage';
    span.textContent = passage.text;
    span.dataset.index = index;
    span.title = `Passage ${{index}} · ${{passage.text.length}} characters`;
    span.addEventListener('click', () => showDetails(index, passage));
    transcript.append(span);
    if (index + 1 < passages.length) {{
      const boundary = document.createElement('i');
      boundary.className = 'boundary';
      boundary.title = passage.strength;
      transcript.append(boundary);
    }}
  }});
}}

function showDetails(index, passage) {{
  document.getElementById('details-content').innerHTML = `
    <h2>Passage ${{index}}</h2>
    <p><b>${{passage.text.length}}</b> characters · atoms ${{passage.startAtom ?? '—'}}–${{passage.endAtom ?? '—'}}</p>
    <p>Boundary after it: <code>${{passage.strength}}</code></p>
    <p>${{escapeHtml(passage.text)}}</p>`;
  document.getElementById('details').showModal();
}}

function escapeHtml(value) {{
  const node = document.createElement('div'); node.textContent = value; return node.innerHTML;
}}

function jumpTo(needle) {{
  const passages = activePassages();
  let index = needle === 'longest'
    ? passages.reduce((best, passage, i) => passage.text.length > passages[best].text.length ? i : best, 0)
    : passages.findIndex(passage => passage.text.includes(needle));
  if (index < 0) return;
  const element = document.querySelector(`[data-index="${{index}}"]`);
  element.classList.add('focus');
  element.scrollIntoView({{block: 'center'}});
  setTimeout(() => element.classList.remove('focus'), 2500);
}}

document.querySelectorAll('[data-preset]').forEach(button => button.addEventListener('click', () => {{
  current = presets[button.dataset.preset]; mode = 'dp';
  document.querySelectorAll('[data-preset]').forEach(item => item.classList.toggle('active', item === button));
  document.querySelectorAll('[data-mode]').forEach(item => item.classList.toggle('active', item.dataset.mode === mode));
  render();
}}));
document.querySelectorAll('[data-mode]').forEach(button => button.addEventListener('click', () => {{
  mode = button.dataset.mode;
  document.querySelectorAll('[data-mode]').forEach(item => item.classList.toggle('active', item === button));
  render();
}}));
document.querySelectorAll('[data-jump]').forEach(button => button.addEventListener('click', () => jumpTo(button.dataset.jump)));
render();
</script>
</body>
</html>
"""
    write_text_atomically(path, document)


def main() -> int:
    arguments = parse_arguments()
    original_windows = load_source_windows(arguments.analysis_run_directory)
    text, source_spans = flatten_source_windows(original_windows)
    atoms = atomize(text, arguments.long_atom_characters)
    boundary_windows = build_boundary_windows(
        atoms, arguments.owned_boundaries_per_window, arguments.context_atoms
    )
    source_batches = batches(boundary_windows, arguments.windows_per_request)
    baseline = read_json(arguments.baseline_segmentation)
    if "passages" not in baseline:
        raise ValueError("baseline segmentation has no passages")
    if "".join(passage["text"] for passage in baseline["passages"]) != text:
        raise ValueError("baseline segmentation does not cover the restored text")

    arguments.output_directory.mkdir(parents=True, exist_ok=True)
    inputs = {
        "format_version": 1,
        "atoms": atoms,
        "boundary_windows": boundary_windows,
    }
    write_json_atomically(arguments.output_directory / "boundary-inputs.json", inputs)
    if arguments.prepare_only:
        print(
            f"Prepared {len(atoms)} atoms, {len(atoms) - 1} candidate boundaries, "
            f"and {len(source_batches)} provider batches."
        )
        return 0

    provider = None
    if not arguments.heuristic_only:
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
        "source_analysis_sha256": sha256(
            arguments.analysis_run_directory / "analysis.json"
        ),
        "baseline_segmentation_sha256": sha256(arguments.baseline_segmentation),
        "model": "heuristic" if provider is None else provider["model"],
        "atom_count": len(atoms),
        "candidate_boundary_count": len(atoms) - 1,
        "boundary_window_count": len(boundary_windows),
        "provider_batch_count": len(source_batches),
        "owned_boundaries_per_window": arguments.owned_boundaries_per_window,
        "context_atoms": arguments.context_atoms,
        "windows_per_request": arguments.windows_per_request,
        "long_atom_characters": arguments.long_atom_characters,
        "preferred_min_characters": arguments.preferred_min_characters,
        "preferred_max_characters": arguments.preferred_max_characters,
        "hard_max_characters": arguments.hard_max_characters,
    }
    manifest_path = arguments.output_directory / "boundary-manifest.json"
    if manifest_path.exists() and read_json(manifest_path) != manifest:
        raise ValueError(f"existing {manifest_path} does not match this experiment")
    write_json_atomically(manifest_path, manifest)

    wall_seconds = 0.0
    checkpoints = []
    if provider is None:
        strengths = heuristic_classifications(atoms)
    else:
        checkpoints, wall_seconds = asyncio.run(
            run_batches(
                source_batches,
                arguments.output_directory,
                provider,
                arguments,
            )
        )
        if len(checkpoints) != len(source_batches):
            print(
                f"Canary complete: {len(checkpoints)}/{len(source_batches)} batches "
                "checkpointed."
            )
            return 0
        strengths = aggregate_classifications(checkpoints, len(atoms) - 1)

    ranges = partition_with_dp(
        atoms,
        strengths,
        arguments.preferred_min_characters,
        arguments.preferred_max_characters,
        arguments.hard_max_characters,
    )
    passages = assemble_passages(atoms, ranges, strengths, source_spans)
    baseline_passages = baseline_with_ranges(baseline)
    diagnostics = {
        "mode": "heuristic" if provider is None else "model",
        "atom_count": len(atoms),
        "candidate_boundary_count": len(strengths),
        "boundary_strength_distribution": {
            strength: strengths.count(strength) for strength in STRENGTHS
        },
        "new": stats(passages),
        "baseline": stats(baseline["passages"]),
        "provider_calls": sum(
            checkpoint["diagnostics"]["provider_calls"] for checkpoint in checkpoints
        ),
        "json_repairs": sum(
            checkpoint["diagnostics"]["json_repairs"] for checkpoint in checkpoints
        ),
        "accepted_http_seconds": sum(
            checkpoint["diagnostics"]["http_seconds"] for checkpoint in checkpoints
        ),
        "last_invocation_wall_seconds": wall_seconds,
    }
    result = {
        "format_version": 1,
        "passages": passages,
        "boundary_strengths": strengths,
        "diagnostics": diagnostics,
    }
    write_json_atomically(
        arguments.output_directory / "boundary-segmentation.json", result
    )
    render_prototype(
        arguments.output_directory / "boundary-dp-prototype.html",
        atoms,
        strengths,
        passages,
        baseline_passages,
        {
            "preferred_min_characters": arguments.preferred_min_characters,
            "preferred_max_characters": arguments.preferred_max_characters,
            "hard_max_characters": arguments.hard_max_characters,
        },
    )
    report = f"""# Boundary + DP segmentation prototype

Question: do model-classified local semantic boundaries plus global length constraints produce more coherent lecture passages than free-form segmentation?

- Mode: {diagnostics["mode"]}
- Atoms: {len(atoms)}
- Candidate boundaries: {len(strengths)}
- Boundary distribution: {json.dumps(diagnostics["boundary_strength_distribution"], ensure_ascii=False)}
- Previous passages: {diagnostics["baseline"]["passage_count"]}; median {diagnostics["baseline"]["median_characters"]}; maximum {diagnostics["baseline"]["maximum_characters"]}
- DP passages: {diagnostics["new"]["passage_count"]}; median {diagnostics["new"]["median_characters"]}; maximum {diagnostics["new"]["maximum_characters"]}
- Provider calls: {diagnostics["provider_calls"]}; JSON repairs: {diagnostics["json_repairs"]}
- Accepted HTTP time: {diagnostics["accepted_http_seconds"]:.1f}s; invocation wall time: {diagnostics["last_invocation_wall_seconds"]:.1f}s

Open `boundary-dp-prototype.html` and use the OOP → generics walkthrough before deciding whether the state model feels right.
"""
    write_text_atomically(arguments.output_directory / "REPORT.md", report)
    print(
        f"Wrote {len(passages)} DP passages and a shareable prototype to "
        f"{arguments.output_directory}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, TypeError, ValueError, RuntimeError) as error:
        print(f"boundary DP prototype: {error}", file=sys.stderr)
        raise SystemExit(1) from error
