# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Compare boundary-first segmentation with a completed lecture on identical text.

Uses the earlier boundary experiment as an experimental baseline, not production
code. All inputs, accepted responses, and timings remain in the output directory.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import statistics
import sys
from pathlib import Path

from prototype_boundary_dp_segmentation import (
    atomize, build_boundary_windows, aggregate_classifications,
    run_batches, partition_with_dp,
)
from compare_importance_bws import write_json_atomically


def text_ranges(passages):
    position = 0
    result = []
    for passage in passages:
        end = position + len(passage["text"])
        result.append({**passage, "start": position, "end": end})
        position = end
    return result


def statistics_for(passages, window_edges):
    lengths = [len(p["text"]) for p in passages]
    return {
        "passages": len(passages), "characters": sum(lengths),
        "median_characters": statistics.median(lengths),
        "minimum_characters": min(lengths), "maximum_characters": max(lengths),
        "under_40_characters": sum(n < 40 for n in lengths),
        "over_450_characters": sum(n > 450 for n in lengths),
        "nonterminal_endings": sum(not p["text"].rstrip().endswith(tuple("。！？!?；;\"”）)")) for p in passages[:-1]),
        "breaks_at_old_window_edges": sum(p["end"] in window_edges for p in passages[:-1]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--key-stdin", action="store_true")
    parser.add_argument("--prompt", type=Path)
    parser.add_argument("--sentence-candidates", action="store_true")
    args = parser.parse_args()
    if args.prompt:
        import prototype_boundary_dp_segmentation as experiment
        experiment.SYSTEM_PROMPT = args.prompt.read_text()
    else:
        import prototype_boundary_dp_segmentation as experiment
    if args.sentence_candidates:
        experiment.PRIMARY_ENDINGS = frozenset("。！？!?；;\n")
    artifact = json.loads((args.baseline / "analysis.json").read_text())
    old = text_ranges(artifact["passages"])
    text = "".join(s.get("text", "") for s in artifact["restored_transcript"]["spans"])
    assert "".join(p["text"] for p in old) == text
    spans = text_ranges([s for s in artifact["restored_transcript"]["spans"] if s["kind"] == "text"])
    position = 0
    edges = set()
    for path in sorted(args.baseline.glob("window-*.json")):
        position += sum(len(p["text"]) for p in json.loads(path.read_text())["analysis"]["passages"])
        edges.add(position)
    assert position == len(text)
    atoms = atomize(text, 180)
    windows = build_boundary_windows(atoms, 48, 8)
    source_batches = [windows[i:i+2] for i in range(0, len(windows), 2)]
    manifest = {
        "source_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "prompt_sha256": hashlib.sha256(experiment.SYSTEM_PROMPT.encode()).hexdigest(),
        "windows": windows,
        "provider": os.getenv("BEYOND_SLIDES_API_BASE_URL"),
        "model": os.getenv("BEYOND_SLIDES_MODEL"),
        "extra_body": json.loads(os.getenv("BEYOND_SLIDES_CHAT_EXTRA_BODY", "{}")),
    }
    args.output.mkdir(parents=True, exist_ok=True)
    path = args.output / "experiment-manifest.json"
    if path.exists() and json.loads(path.read_text()) != manifest:
        raise ValueError("experiment identity changed; use a new output directory")
    write_json_atomically(path, manifest)
    from compare_importance_bws import write_text_atomically
    write_text_atomically(args.output / "prompt.txt", experiment.SYSTEM_PROMPT)
    selected = sorted(set([0, 12, 20, 39, 64, 169, 175, 191, 240, 273] + list(range(0, len(old), 24))))
    audit = {"baseline": statistics_for(old, edges), "sample": [
        {"index": i, "at_window_edge": old[i]["end"] in edges,
         "text": old[i]["text"], "next_text": old[i+1]["text"] if i+1 < len(old) else ""}
        for i in selected
    ]}
    write_json_atomically(args.output / "audit.json", audit)
    print(json.dumps(audit["baseline"], ensure_ascii=False), flush=True)
    if not args.run:
        return
    key = sys.stdin.readline().strip() if args.key_stdin else os.environ["BEYOND_SLIDES_API_KEY"]
    provider = {"url": manifest["provider"].rstrip("/") + "/chat/completions",
                "api_key": key, "model": manifest["model"], "extra_body": manifest["extra_body"]}
    config = argparse.Namespace(concurrency=4, minimum_request_interval=0,
        max_provider_retries=5, max_json_repairs=2, max_provider_requests=80, canary_only=False)
    checkpoints, wall = asyncio.run(run_batches(source_batches, args.output, provider, config))
    strengths = aggregate_classifications(checkpoints, len(atoms)-1)
    audit["timing"] = {"wall_seconds": wall,
        "requests": sum(c["diagnostics"]["provider_attempts"] for c in checkpoints),
        "repairs": sum(c["diagnostics"]["json_repairs"] for c in checkpoints)}
    try:
        ranges = partition_with_dp(atoms, strengths, 80, 280, 450)
    except ValueError as error:
        audit["partition_error"] = str(error)
        write_json_atomically(args.output / "audit.json", audit)
        raise
    passages = []
    for start_atom, end_atom in ranges:
        start, end = atoms[start_atom]["start"], atoms[end_atom-1]["end"]
        supporting = [s for s in spans if s["start"] < end and s["end"] > start]
        passages.append({"text": text[start:end], "start": start, "end": end,
            "source_start": supporting[0]["source_start"], "source_end": supporting[-1]["source_end"]})
    assert "".join(p["text"] for p in passages) == text
    breaks = {end-1 for _,end in ranges}
    audit["boundary_first"] = statistics_for(passages, edges)
    audit["skipped_required_breaks_in_old_prototype"] = [i for i,s in enumerate(strengths) if s == "required_break" and i not in breaks]
    audit["sample_comparison"] = [{"old_index": i,
        "new_passages": [p for p in passages if p["start"] < old[i]["end"] and p["end"] > old[i]["start"]]}
        for i in selected]
    write_json_atomically(args.output / "audit.json", audit)
    write_json_atomically(args.output / "boundary-segmentation.json", {
        "passages": passages, "boundary_strengths": strengths, "atoms": atoms,
    })
    print(json.dumps({k:v for k,v in audit.items() if k not in ("sample", "sample_comparison")}, indent=2))


if __name__ == "__main__":
    main()
