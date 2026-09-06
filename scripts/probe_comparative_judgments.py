# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Small importance-only experiment; does not modify production checkpoints.

uv run scripts/probe_comparative_judgments.py RUN_DIRECTORY OUTPUT_DIRECTORY

Compares global IDs versus inline A/B/C/D, 4 versus 16 groups, two candidate
permutations, and an identical-input repeat. All responses are first attempts:
no repair loop can conceal invalid choices. Credentials are never persisted.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import random
import statistics
import sys
import time
from collections import Counter
from pathlib import Path

import httpx


VARIANTS = ("baseline", "repeat", "reversed", "shuffled")
# Selected and labeled before calling the model. Difficult comparisons have no
# invented unique gold answer. IDs address this particular saved lecture only.
CASES = [
    ("regression", [171, 148, 269, 151], None, None,
     "Original comparison 0: associated types, course aside, struct lifetimes, partial ordering."),
    ("regression", [169, 245, 124, 45], None, None,
     "Original comparison 2: previously selected a passage from another group."),
    ("regression", [222, 188, 38, 288], None, None,
     "Original comparison 6: previously selected a passage from another group."),
    ("regression", [228, 99, 275, 219], None, None,
     "Original comparison 7: failed even after the two repairs."),
    ("clear", [24, 22, 15, 27], 24, 22,
     "Generic types and substitution are central; the personal C++ mastery aside is peripheral."),
    ("clear", [250, 291, 253, 257], 250, 291,
     "Reference validity versus owner lifetime is a core invariant; class dismissal adds no lesson."),
    ("clear", [235, 237, 231, 232], 235, 237,
     "Type erasure explains an invalid match; the isolated 'can match this' fragment carries little alone."),
    ("clear", [36, 10, 34, 35], 36, 10,
     "Compile-time const-generic requirements versus an unfinished transition."),
    ("clear", [9, 8, 0, 14], 9, 0,
     "Duplicated code requires synchronized fixes; the opening recap fragment carries little alone."),
    ("clear", [256, 251, 254, 255], 256, 251,
     "Meaning of an explicit reference lifetime versus an unfinished introductory clause."),
    ("difficult", [24, 56, 218, 250], None, None,
     "All explain core mechanisms in different course topics; no unique most/least is assumed."),
    ("difficult", [235, 245, 269, 263], None, None,
     "Four substantive constraints; type erasure and dynamic compatibility overlap, lifetimes differ."),
    ("difficult", [9, 273, 278, 289], None, None,
     "Transferable design advice versus lifetime-specific guidance; centrality competes with applicability."),
    ("difficult", [37, 38, 36, 35], None, None,
     "Same topic: restriction, its explanation, compile-time rationale, syntax. Avoid rewarding repetition."),
    ("difficult", [256, 260, 261, 287], None, None,
     "Related lifetime explanations plus a confused self-correction; 261 is a plausible least."),
    ("difficult", [171, 269, 283, 272], None, None,
     "Associated-type motivation, struct lifetime requirement, outlives analogy, and subtle NLL example."),
]


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def fixture(run):
    with (run / "model-trace.jsonl").open(encoding="utf-8") as trace:
        records = (json.loads(line) for line in trace)
        original = next(record for record in records
                        if record.get("workflow") == "importance_comparison"
                        and record.get("event") == "request"
                        and record.get("conversation_turn") == 0)
    task = json.loads(original["request"]["messages"][0]["content"][0]["Text"])
    passages = [passage for path in sorted(run.glob("window-*.json"))
                for passage in read_json(path)["analysis"]["passages"]]
    # Detect ordering/fixture mistakes before a network call or interpretation.
    assert len(passages) == 292, "This sample is tied to the 292-passage saved run"
    assert all(passages[p["passage_id"]]["text"] == p["text"] for p in task["passages"])
    cases = [dict(comparison_id=index, category=category, passage_ids=ids,
                  expected_most=most, expected_least=least, reviewer_note=note)
             for index, (category, ids, most, least, note) in enumerate(CASES)]
    selected = sorted({pid for case in cases for pid in case["passage_ids"]})
    return dict(lecture_context=task["lecture_context"], cases=cases,
                passages={str(pid): passages[pid]["text"] for pid in selected},
                reviewer="Codex pre-run assessment, not independent human gold labels",
                source_directory=str(run.resolve()), endpoint=original["endpoint"],
                model=original["model"], global_prompt=original["request"]["system"])


def local_prompt(global_prompt):
    return global_prompt.replace(
        "输入 JSON 包含 lecture_context、comparisons 和 passages。每个 comparison 给出一组 passage_id；passages 提供这些编号对应的讲稿文本。",
        "输入 JSON 包含 lecture_context 和 comparisons。每个 comparison 的 candidates 在同一组内直接给出四段完整文本，分别标记为 A、B、C、D。"
    ).replace(
        "most 和 least 必须来自该 comparison 的 passage_ids，且不能相同。",
        "most 和 least 必须是该 comparison 内的 A、B、C、D 标签，且不能相同。"
    ).replace('"most":12,"least":34', '"most":"A","least":"D"')


def requests_for(sample):
    result = []
    for mode in ("global_ids", "local_labels"):
        for size in (4, 16):
            for variant in VARIANTS:
                for start in range(0, 16, size):
                    groups, mapping = [], {}
                    for case in sample["cases"][start:start + size]:
                        ids = case["passage_ids"].copy()
                        if variant == "reversed":
                            ids.reverse()
                        elif variant == "shuffled":
                            random.Random(20260906 + case["comparison_id"]).shuffle(ids)
                        cid = case["comparison_id"]
                        mapping[str(cid)] = dict(zip("ABCD", ids))
                        if mode == "local_labels":
                            groups.append(dict(comparison_id=cid, candidates={
                                label: sample["passages"][str(pid)]
                                for label, pid in mapping[str(cid)].items()}))
                        else:
                            groups.append(dict(comparison_id=cid, passage_ids=ids))
                    task = dict(lecture_context=sample["lecture_context"], comparisons=groups)
                    if mode == "global_ids":
                        ids = sorted({pid for group in groups for pid in group["passage_ids"]})
                        task["passages"] = [dict(passage_id=pid, text=sample["passages"][str(pid)])
                                            for pid in ids]
                    prompt = sample["global_prompt"]
                    if mode == "local_labels":
                        prompt = local_prompt(prompt)
                    payload = dict(model=sample["model"], temperature=0, max_tokens=16384,
                                   thinking={"type": "disabled"},
                                   response_format={"type": "json_object"}, messages=[
                                       dict(role="system", content=prompt),
                                       dict(role="user", content=json.dumps(task, ensure_ascii=False))])
                    result.append(dict(name=f"{mode}-{size}-{variant}-{start:02d}",
                                       mode=mode, batch_size=size, variant=variant,
                                       mapping=mapping, payload=payload))
    # Interleave conditions rather than confound one format with server time.
    random.Random(20260906).shuffle(result)
    assert len(result) == 40
    return result


def decode(spec, body):
    errors, decisions = [], {}
    try:
        choices = body["choices"]
        if len(choices) != 1 or choices[0]["finish_reason"] != "stop":
            raise ValueError("expected exactly one completed choice")
        items = json.loads(choices[0]["message"]["content"])["comparisons"]
        if not isinstance(items, list):
            raise ValueError("comparisons is not an array")
        counts = Counter(item.get("comparison_id") for item in items if isinstance(item, dict))
        for item in items:
            try:
                cid = item["comparison_id"]
                if type(cid) is not int or str(cid) not in spec["mapping"]:
                    raise ValueError(f"unknown comparison {cid}")
                if counts[cid] != 1:
                    raise ValueError(f"duplicate comparison {cid}")
                mapping = spec["mapping"][str(cid)]
                most, least = item["most"], item["least"]
                if spec["mode"] == "local_labels":
                    most, least = mapping[most], mapping[least]
                if type(most) is not int or type(least) is not int:
                    raise ValueError(f"non-integer passage selection in {cid}")
                if most not in mapping.values() or least not in mapping.values() or most == least:
                    raise ValueError(f"invalid choice in {cid}: most={most}, least={least}")
                decisions[str(cid)] = dict(most=most, least=least)
            except (KeyError, TypeError, ValueError) as error:
                errors.append(str(error))
        for cid in spec["mapping"]:
            if cid not in decisions:
                errors.append(f"missing valid comparison {cid}")
    except (KeyError, IndexError, TypeError, ValueError) as error:
        errors.append(f"invalid response structure: {error}")
    return dict(decisions=decisions, errors=errors)


def agreement(left, right):
    common = sorted(left.keys() & right.keys())
    return dict(pairs=len(common), exact_pair=sum(left[c] == right[c] for c in common),
                most=sum(left[c]["most"] == right[c]["most"] for c in common),
                least=sum(left[c]["least"] == right[c]["least"] for c in common))


def summarize(sample, specs, output):
    records = []
    for spec in specs:
        path = output / (spec["name"] + ".response.json")
        if path.exists():
            record = read_json(path)
            record.update(spec)
            records.append(record)
    results = {}
    for mode in ("global_ids", "local_labels"):
        for size in (4, 16):
            selected = [r for r in records if r["mode"] == mode and r["batch_size"] == size]
            by_variant = {v: {} for v in VARIANTS}
            for record in selected:
                by_variant[record["variant"]].update(record["validation"]["decisions"])
            clear = [c for c in sample["cases"] if c["category"] == "clear"]
            labeled = [(decision, case) for variant in by_variant.values() for case in clear
                       if (decision := variant.get(str(case["comparison_id"]))) is not None]
            times = [r["elapsed_s"] for r in selected]
            results[f"{mode}/{size}"] = dict(
                requests=len(selected), accepted_requests=sum(not r["validation"]["errors"] for r in selected),
                expected_judgments=sum(len(r["mapping"]) for r in selected),
                valid_judgments=sum(len(r["validation"]["decisions"]) for r in selected),
                clear_judgments=len(labeled),
                clear_pair_matches=sum(d == dict(most=c["expected_most"], least=c["expected_least"])
                                       for d, c in labeled),
                clear_most_matches=sum(d["most"] == c["expected_most"] for d, c in labeled),
                clear_least_matches=sum(d["least"] == c["expected_least"] for d, c in labeled),
                candidate_order_agreement={v: agreement(by_variant["baseline"], by_variant[v])
                                           for v in VARIANTS[1:]},
                agreement_by_category={category: {
                    v: agreement(
                        {cid: d for cid, d in by_variant["baseline"].items()
                         if sample["cases"][int(cid)]["category"] == category},
                        {cid: d for cid, d in by_variant[v].items()
                         if sample["cases"][int(cid)]["category"] == category})
                    for v in VARIANTS[1:]}
                    for category in ("clear", "difficult", "regression")},
                http_seconds_sum=sum(times), request_seconds_median=statistics.median(times) if times else None,
                prompt_tokens=sum(r.get("response", {}).get("usage", {}).get("prompt_tokens", 0) for r in selected),
                completion_tokens=sum(r.get("response", {}).get("usage", {}).get("completion_tokens", 0) for r in selected),
                decisions=by_variant)
    across_batch = {mode: {v: agreement(results[f"{mode}/4"]["decisions"][v],
                                      results[f"{mode}/16"]["decisions"][v]) for v in VARIANTS}
                    for mode in ("global_ids", "local_labels")}
    summary = dict(completed_requests=len(records), planned_requests=len(specs),
                   conditions=results, across_batch_agreement=across_batch)
    write_json(output / "summary.json", summary)
    lines = ["# Comparative importance probe", "", "Pre-run assessments are not independent human gold labels.",
             "Invalid judgments are excluded from agreement denominators, never counted as matches.", ""]
    for case in sample["cases"]:
        cid = str(case["comparison_id"])
        lines += [f"## Comparison {cid}: {case['category']}", "", case["reviewer_note"], ""]
        for pid in case["passage_ids"]:
            lines += [f"### Passage {pid}", "", sample["passages"][str(pid)], ""]
        lines += ["| Format / batch | Baseline most/least | Repeat | Reversed | Shuffled |",
                  "| --- | --- | --- | --- | --- |"]
        for condition, data in results.items():
            choices = [data["decisions"][v].get(cid) for v in VARIANTS]
            lines.append(f"| {condition} | " + " | ".join(
                f"{d['most']} / {d['least']}" if d else "INVALID / missing" for d in choices) + " |")
        lines.append("")
    (output / "choice-review.md").write_text("\n".join(lines), encoding="utf-8")
    return summary


async def run_requests(sample, specs, output, api_key):
    started = time.perf_counter()
    semaphore = asyncio.Semaphore(2)
    completed = 0
    async with httpx.AsyncClient(timeout=90.0) as client:
        async def one(spec):
            nonlocal completed
            target = output / (spec["name"] + ".response.json")
            if target.exists():
                return
            async with semaphore:
                write_json(output / (spec["name"] + ".request.json"), spec)
                call_started = time.perf_counter()
                response = await client.post(sample["endpoint"].rstrip("/") + "/chat/completions",
                                             headers={"Authorization": f"Bearer {api_key}"},
                                             json=spec["payload"])
                elapsed = time.perf_counter() - call_started
                try:
                    body = response.json()
                except ValueError:
                    body = dict(raw_body=response.text)
                validation = decode(spec, body) if response.is_success else dict(
                    decisions={}, errors=[f"HTTP {response.status_code}"])
                write_json(target, dict(elapsed_s=elapsed, http_status=response.status_code,
                                        response=body, validation=validation))
                completed += 1
                print(f"{completed}: {spec['name']}: {len(validation['decisions'])}/{len(spec['mapping'])} valid, {elapsed:.2f}s", flush=True)
                if not response.is_success:
                    raise RuntimeError(f"HTTP {response.status_code}; stopping instead of spending more requests")
        # One real experimental request checks access, not an extra smoke call.
        pending = [spec for spec in specs if not (output / (spec["name"] + ".response.json")).exists()]
        if pending:
            await one(pending[0])
            await asyncio.gather(*(one(spec) for spec in pending[1:]))
    elapsed = time.perf_counter() - started
    invocation_path = output / f"invocation-{time.time_ns()}.json"
    write_json(invocation_path, dict(wall_seconds=elapsed, new_requests=completed, concurrency=2))
    print(f"Completed {completed} requests in {elapsed:.2f}s wall-clock", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--summarize-only", action="store_true")
    parser.add_argument("--api-key-stdin", action="store_true", help="Read key from stdin without storing it")
    args = parser.parse_args()
    sample = fixture(args.run_directory)
    specs = requests_for(sample)
    output = args.output_directory
    output.mkdir(parents=True, exist_ok=True)
    fingerprint = hashlib.sha256(json.dumps([sample, specs], ensure_ascii=False).encode()).hexdigest()
    manifest = dict(fingerprint=fingerprint, planned_requests=40, metric="importance", thinking="disabled",
                    batch_sizes=[4, 16], variants=VARIANTS, repairs=0, provider_retries=0)
    # Round-trip tuples before comparing a resumed manifest.
    manifest = json.loads(json.dumps(manifest))
    if (output / "manifest.json").exists() and read_json(output / "manifest.json") != manifest:
        raise ValueError("Experiment changed; use a fresh output directory")
    write_json(output / "manifest.json", manifest)
    write_json(output / "fixture.json", sample)
    if not args.prepare_only and not args.summarize_only:
        api_key = sys.stdin.readline().strip() if args.api_key_stdin else os.environ.get("BEYOND_SLIDES_API_KEY")
        if not api_key:
            raise ValueError("Set BEYOND_SLIDES_API_KEY (the key is not logged)")
        asyncio.run(run_requests(sample, specs, output, api_key))
    summary = summarize(sample, specs, output)
    print(f"Saved {summary['completed_requests']}/{summary['planned_requests']} responses and choice-review.md in {output}")


if __name__ == "__main__":
    main()
