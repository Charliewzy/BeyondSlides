# Passage boundary quality sprint (2026-09-06)

## Controlled experiment

Baseline: `run/real_course/adaptive-pipeline-NTRkf5/analysis.json`, 292 passages
from 146 preparation windows. All experiments reuse its exact restored text:
41,412 Unicode scalar values. Restoration is not rerun. Earlier experiments on
another restored snapshot are not treated as comparable results.

Run the experiment with:

```bash
uv run scripts/evaluate_passage_boundaries.py BASELINE_RUN NEW_OUTPUT_DIRECTORY \
  --run --prompt prompts/passage_boundaries.md --sentence-candidates
```

The standard provider environment variables are required. Alternatively,
`--key-stdin` reads the key from standard input. Experiment requests use GLM-5,
thinking disabled, concurrency four, no fixed spacing, bounded retries and
repairs. Boundary windows own 48 gaps with eight context atoms on either side;
each request contains two windows. The earlier Python experiment supplies the
request harness and DP implementation. This is evaluation code, not a second
production pipeline.

| Variant | Passages | Median / maximum characters | Nonterminal endings | <40 characters | HTTP requests / repairs | Wall time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Existing window-owned preparation | 292 | 131 / 340 | 85 | 16 | Existing checkpoints | Not rerun |
| Earlier boundary prompt, same current source | 233 | 171 / 409 | 2 | 0 | 12 / 0 | 50.29 s |
| Broad stronger coherence instruction | No legal partition | — | — | — | 12 / 0 | Not captured before partition failure |
| Earlier prompt + narrow setup-question reminder | 221 | 197 / 355 | 3 | 1 | 12 / 0 | 49.18 s |
| Same reminder, no colon candidates | 217 | 207 / 315 | 1 | 1 | 12 / 0 | 48.56 s |

The final single short passage is “好，那我们下课。” (eight characters), a
legitimate lecture closing, not an unfinished content fragment. “Nonterminal”
is a punctuation heuristic excluding the lecture's final passage; it is **not
a semantic accuracy score**. Exact concatenation and source-range coverage are
deterministically checked. The original 145 internal preparation-window edges
are forced cuts; the final experiment retains only 20 as passage cuts.

The broad instruction made the model classify whole explanations as
inseparable, including a run exceeding 1,400 characters. The DP rejected the
incompatible 450-character constraint instead of silently violating `continue`.
This variant was rejected. A narrower reminder improved several setup/answer
joins without the same runaway merging. Removing colons from candidate cuts
also prevents a demonstrated split inside `Option<Self::Item>`.

## Qualitative inspection and limits

Before running alternatives, the audit selected ten illustrative problems
plus every 24th baseline passage throughout the lecture (22 unique examples).
This mixes targeted regressions with regularly spaced ordinary material, but
is not a blinded independent evaluation or a multi-lecture benchmark.

Clear improvements include:

- The 17-character C++ template introduction joins its actual explanation.
- The OOP example retains its inheritance/polymorphism explanation together.
- The associated-types introduction joins the Graph type parameters instead of
  ending at a processing-window edge.
- The orphan-rule introduction no longer consists solely of “推论就是”.
- The lifetime advice keeps the recommendation separate from its useful
  borrowing-without-copying exception, each with explanatory content.

Remaining issues must remain visible in the review, not be called solved:

- Some setup questions still attach backward; the static-dispatch comparison
  and list example are examples in the final experimental partition.
- The final experiment has one comma-ending cut introducing reference counting.
- Ordinary material sometimes receives different but debatable boundaries,
  such as splitting a Clone method explanation from its default implementation.
- Joining text cannot repair mistakes already present in restoration, missing
  code, or awkward grammar. No source words are rewritten in this experiment.
- Fewer fragments do not prove more accurate comparative judgments. New scores
  must be computed on the new passages, not transferred from old IDs.

Artifacts: `run/real_course/passage-quality-20260906/experiment{,-v3,-v4,-v5}/`.
Accepted responses and per-request diagnostics are retained. The final variant
also stores its exact prompt, task inputs, source hash, audit, and partition.
The failed trial's wall time was not saved by the initial harness; the harness
now records timing before attempting the DP so this failure does not recur.

## Integration decision

Integrate the no-colon candidate method as an opt-in passage-preparation mode,
retaining the old window-owned mode for comparison. Requests classify gaps;
Rust selects a global partition, so request boundaries cannot force cuts.
Keep the semantic `continue` constraint and enforce `required_break` as a hard
constraint rather than merely rewarding it. Infeasible constraints must be
reported, with classifications preserved. Do not silently relax them.

This mode prepares passages directly from restoration, without running the old
tool-calling preparation first. It infers slide positions for the new passages;
novelty continues to retrieve its own slide evidence. It does not transfer old
summaries, related-slide judgments, connection strengths, or comparative scores
onto a different partition. Existing legacy fields use empty/zero placeholders
where that stage made no judgment; importance and novelty are ranked afterward.
