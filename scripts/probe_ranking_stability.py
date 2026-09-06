# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Importance-only ranking stability experiment on saved prepared passages.

Runs three disjoint seeded schedules, eight rounds each, inline A/B/C/D,
16 quartets/request, thinking disabled, concurrency two, no fixed pacing.
Does not write production artifacts. No retries or repairs conceal failures.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import itertools
import json
import math
import os
import random
import statistics
import sys
import time
from collections import Counter
from fractions import Fraction
from pathlib import Path

import httpx

from probe_comparative_judgments import decode, fixture, local_prompt, read_json, write_json


SEEDS = (20260905, 20270905, 20280905)
ROUNDS = 8
BATCH_SIZE = 16
MASK64 = (1 << 64) - 1


def rust_shuffle(values, seed):
    """Exact 64-bit port of src/comparative_ranking.rs::shuffle."""
    state = seed
    for end in range(len(values) - 1, 0, -1):
        state = (state + 0x9E3779B97F4A7C15) & MASK64
        random_value = state
        random_value = ((random_value ^ (random_value >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        random_value = ((random_value ^ (random_value >> 27)) * 0x94D049BB133111EB) & MASK64
        random_value ^= random_value >> 31
        other = random_value % (end + 1)
        values[end], values[other] = values[other], values[end]


def comparison_plan(passage_count, rounds, seed):
    # This study requires exact equal exposure, without production's padding
    # for passage counts not divisible by four.
    if passage_count < 4 or passage_count % 4:
        raise ValueError("This experiment requires a positive multiple of four passages")
    comparisons = []
    for round_index in range(rounds):
        ids = list(range(passage_count))
        rust_shuffle(ids, (seed + round_index) & MASK64)
        for start in range(0, passage_count, 4):
            comparisons.append(dict(comparison_id=len(comparisons), round_index=round_index,
                                    passage_ids=ids[start:start + 4]))
    return comparisons


def load_sample(run):
    previous = fixture(run)  # Also verifies source ordering against the saved request.
    passages = [p for path in sorted(run.glob("window-*.json"))
                for p in read_json(path)["analysis"]["passages"]]
    return dict(source_directory=str(run.resolve()), passage_count=len(passages),
                passages=passages, lecture_context=previous["lecture_context"],
                endpoint=previous["endpoint"], model=previous["model"],
                prompt=local_prompt(previous["global_prompt"]))


def request_plan(sample):
    requests, schedules = [], []
    for run_index, seed in enumerate(SEEDS):
        comparisons = comparison_plan(sample["passage_count"], ROUNDS, seed)
        schedules.append(dict(run_index=run_index, seed=seed, comparisons=comparisons))
        for start in range(0, len(comparisons), BATCH_SIZE):
            mapping, groups = {}, []
            for comparison in comparisons[start:start + BATCH_SIZE]:
                cid = comparison["comparison_id"]
                mapping[str(cid)] = dict(zip("ABCD", comparison["passage_ids"]))
                groups.append(dict(comparison_id=cid, candidates={
                    label: sample["passages"][pid]["text"]
                    for label, pid in mapping[str(cid)].items()}))
            task = dict(lecture_context=sample["lecture_context"], comparisons=groups)
            payload = dict(model=sample["model"], temperature=0, max_tokens=16384,
                           thinking={"type": "disabled"}, response_format={"type": "json_object"},
                           messages=[dict(role="system", content=sample["prompt"]),
                                     dict(role="user", content=json.dumps(task, ensure_ascii=False))])
            requests.append(dict(name=f"run-{run_index + 1}-batch-{start // BATCH_SIZE:03d}",
                                 run_index=run_index, seed=seed, mode="local_labels",
                                 mapping=mapping, payload=payload))
    random.Random(20260906).shuffle(requests)
    return requests, schedules


def rank_counts(counts):
    """Same rational score ordering and integer percentile rounding as Rust."""
    if any(c["comparisons"] == 0 for c in counts):
        raise ValueError("Cannot rank a passage without comparison evidence")
    balances = [Fraction(c["most"] - c["least"], c["comparisons"]) for c in counts]
    ordered = sorted(range(len(counts)), key=balances.__getitem__)
    result = [None] * len(counts)
    start = 0
    while start < len(ordered):
        end = start + 1
        while end < len(ordered) and balances[ordered[start]] == balances[ordered[end]]:
            end += 1
        denominator = len(ordered) - 1
        basis_points = ((start + end - 1) * 5000 + denominator // 2) // denominator
        average_rank = (start + end - 1) / 2 + 1
        for pid in ordered[start:end]:
            result[pid] = dict(passage_id=pid, **counts[pid], score=float(balances[pid]),
                               average_rank=average_rank, percentile_basis_points=basis_points,
                               display_level=min(basis_points // 2000 + 1, 5))
        start = end
    return result


def aggregate(passage_count, comparisons, decisions, rounds):
    counts = [dict(comparisons=0, most=0, least=0) for _ in range(passage_count)]
    for comparison in comparisons:
        if comparison["round_index"] >= rounds:
            continue
        cid = str(comparison["comparison_id"])
        if cid not in decisions:
            raise ValueError(f"Missing comparison {cid}; incomplete schedules are not ranked")
        decision = decisions[cid]
        ids = comparison["passage_ids"]
        if decision["most"] not in ids or decision["least"] not in ids or decision["most"] == decision["least"]:
            raise ValueError(f"Invalid comparison {cid}")
        for pid in ids:
            counts[pid]["comparisons"] += 1
        counts[decision["most"]]["most"] += 1
        counts[decision["least"]]["least"] += 1
    if any(c["comparisons"] != rounds for c in counts):
        raise ValueError("Unequal comparison exposure")
    return rank_counts(counts)


def fractional_top_k(ranking, k, *, bottom=False):
    """Share cutoff membership equally among ties instead of breaking by ID."""
    ordered = sorted(ranking, key=lambda r: r["score"], reverse=not bottom)
    cutoff = ordered[k - 1]["score"]
    strict = [r for r in ordered if r["score"] < cutoff] if bottom else [
        r for r in ordered if r["score"] > cutoff]
    tied = [r for r in ordered if r["score"] == cutoff]
    weights = {r["passage_id"]: 1.0 for r in strict}
    weights.update({r["passage_id"]: (k - len(strict)) / len(tied) for r in tied})
    return weights


def compare_rankings(left, right):
    count = len(left)
    k = math.ceil(count * 0.2)
    deltas = [abs(a["percentile_basis_points"] - b["percentile_basis_points"]) / 100
              for a, b in zip(left, right)]
    overlap = {}
    for bottom in (False, True):
        a, b = fractional_top_k(left, k, bottom=bottom), fractional_top_k(right, k, bottom=bottom)
        overlap["bottom" if bottom else "top"] = sum(min(a.get(pid, 0), b.get(pid, 0)) for pid in range(count)) / k
    levels_a, levels_b = [r["display_level"] for r in left], [r["display_level"] for r in right]
    top_a, top_b = {r["passage_id"] for r in left if r["display_level"] == 5}, {
        r["passage_id"] for r in right if r["display_level"] == 5}
    ranks_a, ranks_b = [r["average_rank"] for r in left], [r["average_rank"] for r in right]
    correlation = statistics.correlation(ranks_a, ranks_b) if len(set(ranks_a)) > 1 and len(set(ranks_b)) > 1 else None
    return dict(spearman=correlation,
                top_k=k, fractional_top_overlap=overlap["top"], fractional_bottom_overlap=overlap["bottom"],
                same_display_level=sum(a == b for a, b in zip(levels_a, levels_b)),
                within_one_level=sum(abs(a - b) <= 1 for a, b in zip(levels_a, levels_b)),
                threshold_flips={str(level): sum((a >= level) != (b >= level) for a, b in zip(levels_a, levels_b))
                                 for level in (2, 3, 4, 5)},
                extreme_swaps=sum((a == 5 and b == 1) or (a == 1 and b == 5)
                                  for a, b in zip(levels_a, levels_b)),
                median_percentile_change=statistics.median(deltas), max_percentile_change=max(deltas),
                top_level_set_sizes=[len(top_a), len(top_b)], top_level_intersection=len(top_a & top_b),
                top_level_jaccard=len(top_a & top_b) / len(top_a | top_b) if top_a | top_b else None)


async def run_requests(sample, requests, output, api_key):
    started = time.perf_counter()
    pending = []
    for spec in requests:
        path = output / (spec["name"] + ".response.json")
        if path.exists():
            if read_json(path)["validation"]["errors"]:
                raise ValueError(f"Saved failure in {path}; reassess before continuing")
        else:
            pending.append(spec)
    failures, completed = [], []
    try:
        async with httpx.AsyncClient(timeout=90.0) as client:
            async def one(spec):
                write_json(output / (spec["name"] + ".request.json"), spec)
                call_started = time.perf_counter()
                record = {}
                try:
                    response = await client.post(sample["endpoint"].rstrip("/") + "/chat/completions",
                                                 headers={"Authorization": f"Bearer {api_key}"},
                                                 json=spec["payload"])
                    try:
                        body = response.json()
                    except ValueError:
                        body = dict(raw_body=response.text)
                    record.update(http_status=response.status_code, response=body,
                                  validation=decode(spec, body) if response.is_success else
                                  dict(decisions={}, errors=[f"HTTP {response.status_code}"]))
                except httpx.HTTPError as error:
                    record.update(transport_error=type(error).__name__,
                                  validation=dict(decisions={}, errors=[type(error).__name__]))
                record["elapsed_s"] = time.perf_counter() - call_started
                write_json(output / (spec["name"] + ".response.json"), record)
                completed.append(spec["name"])
                errors = record["validation"]["errors"]
                if errors:
                    failures.append(dict(request=spec["name"], errors=errors))
                print(f"{len(completed)}/{len(pending)} {spec['name']}: "
                      f"{len(record['validation']['decisions'])}/{len(spec['mapping'])} valid, "
                      f"{record['elapsed_s']:.2f}s", flush=True)

            if pending:
                await one(pending[0])  # Real experimental request, not extra paid preflight.
                work = iter(pending[1:])

                async def worker():
                    while not failures:
                        spec = next(work, None)
                        if spec is None:
                            return
                        await one(spec)

                await asyncio.gather(worker(), worker())
    finally:
        elapsed = time.perf_counter() - started
        write_json(output / f"invocation-{time.time_ns()}.json", dict(
            wall_seconds=elapsed, completed_requests=completed, concurrency=2,
            minimum_request_interval_s=0, failures=failures))
        print(f"Finished {len(completed)} requests in {elapsed:.2f}s wall-clock", flush=True)
    if failures:
        raise ValueError(f"Stopped on {failures[0]['request']}: {failures[0]['errors']}")


def summarize(sample, requests, schedules, output):
    records, decisions = [], [{} for _ in SEEDS]
    position_counts = [dict(most=Counter(), least=Counter()) for _ in SEEDS]
    for spec in requests:
        path = output / (spec["name"] + ".response.json")
        if not path.exists():
            continue
        record = read_json(path)
        records.append(record)
        if record["validation"]["errors"]:
            continue
        decisions[spec["run_index"]].update(record["validation"]["decisions"])
        for cid, decision in record["validation"]["decisions"].items():
            labels = {pid: label for label, pid in spec["mapping"][cid].items()}
            for metric in ("most", "least"):
                position_counts[spec["run_index"]][metric][labels[decision[metric]]] += 1
    summary = dict(completed_requests=len(records), expected_requests=len(requests),
                   accepted_requests=sum(not r["validation"]["errors"] for r in records),
                   completed_comparisons=[len(d) for d in decisions])
    write_json(output / "summary.json", summary)
    if any(len(d) != len(s["comparisons"]) for d, s in zip(decisions, schedules)):
        return summary

    ranked_by_rounds = {}
    for rounds in (2, 4, 8):
        ranked = [aggregate(sample["passage_count"], s["comparisons"], d, rounds)
                  for s, d in zip(schedules, decisions)]
        ranked_by_rounds[str(rounds)] = ranked
        summary[f"pairs_at_{rounds}_rounds"] = {
            f"{a + 1}-{b + 1}": compare_rankings(ranked[a], ranked[b])
            for a, b in itertools.combinations(range(3), 2)}
    write_json(output / "rankings.json", ranked_by_rounds)
    rankings = ranked_by_rounds["8"]
    summary["level_counts"] = [dict(sorted(Counter(r["display_level"] for r in ranks).items())) for ranks in rankings]
    summary["median_request_seconds"] = statistics.median(r["elapsed_s"] for r in records)
    summary["p95_request_seconds"] = sorted(r["elapsed_s"] for r in records)[int(len(records) * .95)]
    summary["max_request_seconds"] = max(r["elapsed_s"] for r in records)
    summary["prompt_tokens"] = sum(r["response"].get("usage", {}).get("prompt_tokens", 0) for r in records)
    summary["completion_tokens"] = sum(r["response"].get("usage", {}).get("completion_tokens", 0) for r in records)
    summary["invocations"] = [read_json(path) for path in sorted(output.glob("invocation-*.json"))]
    movements = []
    for pid, passage in enumerate(sample["passages"]):
        percentiles = [ranks[pid]["percentile_basis_points"] / 100 for ranks in rankings]
        movements.append(dict(passage_id=pid, text=passage["text"], percentiles=percentiles,
                              levels=[ranks[pid]["display_level"] for ranks in rankings],
                              percentile_range=max(percentiles) - min(percentiles),
                              mean_percentile=statistics.mean(percentiles)))
    summary["all_runs_same_level"] = sum(len(set(m["levels"])) == 1 for m in movements)
    summary["all_runs_within_one_level"] = sum(max(m["levels"]) - min(m["levels"]) <= 1 for m in movements)
    summary["all_runs_threshold_flips"] = {
        str(level): sum(len({value >= level for value in m["levels"]}) > 1 for m in movements)
        for level in (2, 3, 4, 5)}
    summary["position_selection_counts"] = position_counts
    summary["top_level_all_runs"] = [m["passage_id"] for m in movements if min(m["levels"]) == 5]
    summary["bottom_level_all_runs"] = [m["passage_id"] for m in movements if max(m["levels"]) == 1]
    summary["largest_movements"] = sorted(movements, key=lambda m: -m["percentile_range"])[:20]
    write_json(output / "summary.json", summary)
    write_json(output / "passage-stability.json", movements)
    lines = ["# Passage ranking review", "", "Percentiles are lecture-relative, not accuracy or confidence.", ""]
    for title, rows in [
        ("Largest changes", sorted(movements, key=lambda m: -m["percentile_range"])[:20]),
        ("Consistently high", sorted([m for m in movements if min(m["levels"]) == 5], key=lambda m: -m["mean_percentile"])[:12]),
        ("Consistently low", sorted([m for m in movements if max(m["levels"]) == 1], key=lambda m: m["mean_percentile"])[:12]),
    ]:
        lines += [f"## {title}", ""]
        for m in rows:
            lines += [f"### Passage {m['passage_id']}", "",
                      f"Percentiles: {m['percentiles']}; display levels: {m['levels']}.", "", m["text"], ""]
    lines += ["## All passages", "", "| Passage | Run 1 percentile | Run 2 | Run 3 | Levels |",
              "| ---: | ---: | ---: | ---: | --- |"]
    for m in movements:
        lines.append(f"| {m['passage_id']} | " + " | ".join(str(p) for p in m["percentiles"]) + f" | {m['levels']} |")
    (output / "passage-review.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--summarize-only", action="store_true")
    parser.add_argument("--api-key-stdin", action="store_true")
    args = parser.parse_args()
    sample = load_sample(args.run_directory)
    requests, schedules = request_plan(sample)
    manifest = dict(fingerprint=hashlib.sha256(json.dumps([sample, requests], ensure_ascii=False).encode()).hexdigest(),
                    metric="importance", seeds=list(SEEDS), rounds=ROUNDS, batch_size=BATCH_SIZE,
                    expected_requests=len(requests), thinking="disabled", concurrency=2,
                    minimum_request_interval_s=0, provider_retries=0, final_answer_repairs=0)
    output = args.output_directory
    output.mkdir(parents=True, exist_ok=True)
    if (output / "manifest.json").exists() and read_json(output / "manifest.json") != manifest:
        raise ValueError("Experiment changed; choose a new output directory")
    write_json(output / "manifest.json", manifest)
    write_json(output / "fixture.json", sample)
    write_json(output / "schedules.json", schedules)
    if not args.prepare_only and not args.summarize_only:
        key = sys.stdin.readline().strip() if args.api_key_stdin else os.environ.get("BEYOND_SLIDES_API_KEY")
        if not key:
            raise ValueError("Set BEYOND_SLIDES_API_KEY; credentials are not logged")
        try:
            asyncio.run(run_requests(sample, requests, output, key))
        finally:
            summarize(sample, requests, schedules, output)
    summary = summarize(sample, requests, schedules, output)
    print(f"Saved {summary['completed_requests']}/{len(requests)} requests in {output}")


if __name__ == "__main__":
    main()
