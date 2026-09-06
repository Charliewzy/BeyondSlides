# Real-lecture comparative prototype (2026-09-06)

## Scope

Completed the production Rust comparative pipeline against the existing
`run/real_course/glm-5-comparative-analysis-restoration-contract/` run. Reused
all 150 restored windows and all 146 prepared windows, containing 292 passages.
This was a resume, **not a fresh end-to-end timing measurement**.

Both metrics now receive inline A/B/C/D candidates; Rust maps choices to
canonical passage IDs before saving them. Novelty retains pre-fetched evidence
from the 80-slide deck. No comparison tool calls were needed. Eight shuffled
rounds give every passage eight comparisons per metric: 584 quartets for
importance and another 584 for novelty.

Provider: the existing Tsinghua OpenAI-compatible endpoint, `glm-5`, thinking
disabled, temperature 0, JSON-object output, 16,384 maximum output tokens.
Execution used concurrency **2**, request spacing **0 ms**, bounded provider
retries, and at most two structured-answer repair attempts.

## Completion and timing

The complete resumed command, including local retrieval setup, comparisons,
slide rendering, audio asset staging, and report generation, took **159.90
seconds (2 minutes 40 seconds)** according to `/usr/bin/time -p`. The ranking
progress timer displayed **2 minutes 33 seconds**. These figures do not include
compilation or browser verification.

| Metric | Completed batches | Quartets | Requests, including repairs | Repaired batches | Provider errors |
| --- | ---: | ---: | ---: | ---: | ---: |
| Importance | 37 | 584 | 37 | 0 | 0 |
| Novelty | 73 | 584 | 75 | 2 | 0 |

The two rejected novelty responses selected the same candidate as both `most`
and `least` (comparison IDs 69 and 341). Each was corrected by one repair
request. They were not accepted silently. There were no unknown-candidate
failures or exhausted repair budgets. Mean provider-response latency was
2.99 seconds for importance and 2.54 seconds for novelty, including the repair
responses; these are per-request latencies, not wall time per stage.

## Resume integrity

- SHA-256 checks before and after confirmed all **296 upstream window
  checkpoints were byte-for-byte unchanged**.
- The restoration trace remained at 453 records; it received no new requests.
- Only comparative requests were appended to the analysis trace, starting at
  event index 695: 112 requests, 112 responses, and 112 validation records.
  Earlier failed experiments in the same trace were excluded from these counts.
- The root's original version-6 manifest was preserved. Comparison prompt
  changes and the old concurrency-4/five-second pacing did not invalidate
  passage-preparation results.
- A subsequent run using concurrency 3, 25 ms spacing, an unusable API key, and
  an unreachable HTTPS proxy restored all **110 comparison batches** as well
  as both upstream stages. It completed in 12.48 seconds with no new trace
  records. Scheduling was then returned to the default 2/0 settings.

Comparison checkpoints live in per-metric configuration-hashed directories,
each with its own manifest. Changed metric prompts produce a new namespace
without deleting previous comparisons. Unit tests verify that source,
restoration/preparation prompt, model, and other relevant semantic changes
still reject upstream reuse; passage/grouping/evidence dependencies bind the
comparisons. Checkpoint domain validation remains mandatory.

## Report verification

Output: `run/real_course/glm-5-comparative-analysis-restoration-contract/report.html`.
Its neighboring `report.assets/` contains all 80 slide images and the existing
audio. All 292 passages obtained token-level playback timing; none needed a
coarse-timing fallback in this run.

Local Chromium/Playwright checks verified:

- all 292 passages and 80 slide images render, and the audio metadata loads;
- clicking a passage selects it, centers its inferred aligned slide, and plays
  audio at the projected passage start;
- manually browsing slides changes the enlarged viewing slide independently
  of the blue aligned-slide state;
- clicking a slide scrolls to its transcript and highlights all matching
  passages without starting audio;
- slide hover temporarily highlights matching passages, and passage hover
  highlights the complete passage;
- importance/novelty sliders switch between all/none and threshold emphasis,
  and remain fixed in the upper-right corner;
- continuous playback crosses a passage boundary, single-passage playback
  stops at its boundary, and the current-audio passage updates;
- `?prototype=minimap` enables the existing binary minimap, including navigation
  and dragging its viewport without starting audio;
- no page-level JavaScript errors occurred.

The local audit script, JSON results, and viewport screenshots are saved beside
the report as `verify_report.py`, `browser-verification.json`,
`report-desktop.png`, and `report-minimap.png`. These are ignored run artifacts.

## Interpretation

| Display level | Importance passages | Novelty passages |
| ---: | ---: | ---: |
| 1 | 66 | 68 |
| 2 | 43 | 39 |
| 3 | 74 | 81 |
| 4 | 56 | 39 |
| 5 | 53 | 65 |

These are percentile bands with ties, not calibrated absolute judgments or
evidence of semantic accuracy. This run demonstrates a functioning prototype
and much lower scheduling overhead; it does not establish optimal concurrency
for other providers. Existing prepared passage boundaries were preserved,
including imperfect fragments. The earlier importance stability experiment
still applies, and novelty choices have not received a human accuracy audit.

See [running instructions](../running.md) for environment variables, the full
command, assets, and stage-aware resume behavior.
