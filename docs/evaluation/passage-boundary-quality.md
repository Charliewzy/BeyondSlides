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
plus every 24th baseline passage throughout the lecture (21 unique examples).
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
onto a different partition. Unassessed connection strength is `null` (distinct
from a measured zero); optional text fields and related-slide lists are empty.
Importance and novelty are ranked afterward.

## Production run and final artifacts

The Rust run uses the selected prompt/candidate policy, the shared adaptive
scheduler (starts at two, ceiling eight, zero spacing floor), GLM-5 with thinking
disabled, and the existing restoration. It produced **222 passages** rather
than the experimental 217: these were fresh model classifications, not imported
experimental answers. This difference demonstrates remaining model variability.
Replaying the production classifications through the Python reference DP
produced the **identical partition** to Rust (1,064 atoms / 1,063 candidate gaps).

| Measurement | Existing report | Final boundary report |
| --- | ---: | ---: |
| Passages | 292 | 222 |
| Median characters | 131 | 193.5 |
| Maximum characters | 340 | 356 |
| Nonterminal endings, excluding lecture end | 85 | 0 |
| Passages below 40 characters | 16 | 1 |
| Cuts coinciding with old processing-window edges | 145 | 24 |
| Authoritative characters preserved | 41,412 | 41,412 |

The final short passage is the twelve-character lecture closing
“那我们今天课就讲到这儿。” before the announcement of tomorrow's topic. Zero
nonterminal endings does not mean zero semantic defects: the static-dispatch
setup question still attaches backward, as does one multiple-lifetime example
introduction. The review includes these cases. The associated-types setup is
shorter than in the experimental partition, but its unfinished syntax is no
longer caused by a request-window edge.

### Time and request accounting

The initial complete Rust command took **139.66 seconds (2m20s)** according to
`/usr/bin/time -p`, including retrieval setup, boundary preparation, both metrics,
asset staging and rendering. It reused 150 restored checkpoints; this is **not
a raw-ASR-to-report timing**. It sent 12 boundary requests, 28 importance
requests and 56 novelty requests, with no repairs or provider failures.

Boundary classification spans 83.93 seconds between its first request and final
validation trace event. Importance spans 25.34 seconds and novelty 39.43 seconds;
the metrics overlap, so these durations must not be added. Local preparation
diagnostics use a separate monotonic interval including loading/planning work.
The first request ran alone as a canary, and the adaptive scheduler began at
two; this production execution is intentionally more conservative than the
fixed-four experimental cells.

During report review, unassessed connection strength was corrected from a
legacy zero placeholder to `null`, displayed as “未评估”. That artifact change
selected new comparison namespaces under the conservative exact-passage hash.
Restoration and all 12 boundary batches were reused; 84 comparison batches were
rerun (85 HTTP requests, one repaired importance response selecting the same
candidate as both most and least, no provider failures). The final report uses
these latter comparisons. Its 896 best–worst judgments give every passage eight
comparisons for each metric. The original namespaces and traces remain intact.
The follow-up command's wall time was not successfully captured; its comparative
trace spans 50.72 seconds for novelty and 35.30 seconds for importance, overlapping.

A final offline resume, using an invalid key, unreachable HTTPS proxy, fixed
concurrency three and 25 ms spacing, took **13.41 seconds**. It reused all
150 restoration, 12 boundary and 84 final comparison checkpoints and made zero
HTTP requests; the analysis trace remained at 543 records. The initial and
final live scheduler snapshots are preserved separately from the latest resume
snapshot. SHA-256 checks confirmed all 296 original upstream checkpoints in
both baseline directories remain unchanged; copied restoration checkpoints
also match byte-for-byte.

A separate scratch copy of the old window-mode run also resumed offline in
7.05 seconds: all 150 restoration, 146 preparation and 110 comparison
checkpoints were reused. Its trace remained byte-identical to the baseline,
confirming that the new preparation identity representation and optional
connection score did not invalidate legacy comparisons. This smoke test omitted
media rendering and is not directly comparable to the media-enabled timings.

### Review and verification

Open `run/real_course/passage-quality-20260906/review.html` first. It offers:

- two complete chronological partitions, linked by authoritative source range;
- 21 targeted/regularly spaced sample jumps and full-lecture position navigation;
- orange markers for old forced processing-window cuts;
- optional importance/novelty emphasis, off initially so boundaries can be read
  without score emphasis;
- explicit audio playback of the selected old or new passage;
- links to both full interactive reports.

The final interactive report is
`run/real_course/passage-quality-20260906/production/report.html`.
All 80 slide images load, and all 222 passages have token-level playback timing.
Browser checks passed for passage/slide selection in both directions, independent
viewing versus aligned slides, hover, fixed threshold controls, continuous and
single-passage audio modes, and minimap navigation/viewport dragging. The paired
review passed overlap highlighting, sample/range navigation without autoplay,
score toggles, explicit audio, and narrow-screen overflow checks; neither page
reported JavaScript errors. Screenshots and audit JSON sit beside the reports.

Rust all-target/all-feature tests, formatting and Clippy with warnings denied
passed. Python tests cover review source equality, timing count consistency,
and HTML-safe embedded source. Deterministic Rust tests cover cross-request
passages, Unicode byte ranges, required/forbidden gaps, infeasibility, malformed
or mismatched checkpoints, source provenance, provider repairs and trace labels.

The codebase-design skill guided the segmentation interface; the domain-modeling
skill recorded its distinction from copied-text projection in ADR-0006. The
old mode remains the default pending the user's qualitative review. This sprint
supports a better working prototype, not a claim of optimal segmentation or
proven importance/novelty accuracy.
