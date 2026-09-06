# /// script
# requires-python = ">=3.12"
# dependencies = ["httpx>=0.28,<0.29"]
# ///
"""Render a local before/after review of two partitions of identical source text.

Usage: uv run scripts/render_passage_boundary_review.py OLD_RUN NEW_RUN OUTPUT.html
No provider requests; model judgments are displayed, not independently validated.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import quote

from evaluate_passage_boundaries import text_ranges, statistics_for
from compare_importance_bws import write_json_atomically, write_text_atomically


class ReportTiming(HTMLParser):
    def __init__(self):
        super().__init__()
        self.passages = []
        self.audio = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "data-passage" in attrs:
            self.passages.append({
                "audio_start_ms": int(attrs["data-audio-start-ms"]),
                "audio_end_ms": int(attrs["data-audio-end-ms"]),
                "timestamp": attrs["data-time"],
            })
        if tag == "audio":
            self.audio = attrs.get("src")


def load_run(directory):
    artifact = json.loads((directory / "analysis.json").read_text())
    source = "".join(s.get("text", "") for s in artifact["restored_transcript"]["spans"])
    passages = text_ranges(artifact["passages"])
    if not passages or "".join(p["text"] for p in passages) != source:
        raise ValueError(f"{directory} does not partition its restored transcript")
    timing = ReportTiming()
    timing.feed((directory / "report.html").read_text())
    if len(timing.passages) != len(passages):
        raise ValueError("rendered report and analysis passage counts disagree")
    for passage, interval in zip(passages, timing.passages, strict=True):
        passage.update(interval)
    return source, passages, timing.audio


def build_review(old_directory, new_directory, output):
    old_source, old, _ = load_run(old_directory)
    new_source, new, audio = load_run(new_directory)
    if old_source != new_source:
        raise ValueError("before/after comparison requires exactly the same authoritative text")
    edges, position = set(), 0
    for path in sorted(old_directory.glob("window-*.json")):
        checkpoint = json.loads(path.read_text())
        position += sum(len(p["text"]) for p in checkpoint["analysis"]["passages"])
        edges.add(position)
    if position and position != len(old_source):
        raise ValueError("baseline windows do not cover the baseline source")
    for passage in old:
        passage["window_edge"] = passage["end"] in edges
    chosen = sorted(i for i in set([0, 12, 20, 39, 64, 169, 175, 191, 240, 273] + list(range(0, len(old), 24))) if i < len(old))
    output.parent.mkdir(parents=True, exist_ok=True)
    relative = lambda path: quote(os.path.relpath(path.resolve(), output.parent.resolve()).replace(os.sep, "/"), safe="/")
    data = {
        "old": old, "new": new, "characters": len(old_source),
        "samples": chosen, "source_sha256": hashlib.sha256(old_source.encode()).hexdigest(),
        "stats": {"old": statistics_for(old, edges), "new": statistics_for(new, edges)},
        "reports": {"old": relative(old_directory / "report.html"), "new": relative(new_directory / "report.html")},
        "audio": relative(new_directory / audio) if audio else None,
    }
    template = Path(__file__).with_name("templates").joinpath("passage_boundary_review.html").read_text()
    encoded = json.dumps(data, ensure_ascii=False).replace("<", "\\u003c").replace("\u2028", "\\u2028").replace("\u2029", "\\u2029")
    write_text_atomically(output, template.replace("__REVIEW_DATA__", encoded))
    write_json_atomically(output.with_suffix(".json"), {k:v for k,v in data.items() if k not in ("old", "new")})
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("old_directory", type=Path)
    parser.add_argument("new_directory", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    data = build_review(args.old_directory, args.new_directory, args.output)
    print(json.dumps(data["stats"], indent=2))
    print(f"Wrote {args.output}")


if __name__ == "__main__":
    main()
