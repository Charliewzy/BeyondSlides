# Importance ranking stability across comparison schedules

## Scope and method

This experiment reuses all 292 prepared passages from
`run/real_course/glm-5-comparative-analysis-restoration-contract/` unchanged.
It does not rerun restoration, retrieval, or passage preparation, and it does
not evaluate novelty. Production Rust code and checkpoints are not modified.

Provider: the source run's Tsinghua OpenAI-compatible endpoint, `glm-5`, thinking
disabled, temperature 0, JSON-object output, 16,384 maximum output tokens.
Inputs use inline A/B/C/D candidates with the previous importance rubric.

Three schedules use seeds 20260905, 20270905, and 20280905. Each contains eight
rounds of 73 quartets, giving every passage exactly eight comparisons. The
schedule generator reproduces the Rust shuffle; the seed ranges do not overlap
(using seeds one apart would have shared seven of eight rounds). Both group
membership and candidate positions vary, so this measures their combined
effect plus model nondeterminism, not each cause independently. Candidate
positions are randomized, not forced to be balanced for each passage.

Each schedule requires 37 requests: 36 batches of 16 quartets and one of eight.
The 111 requests are interleaved in a seeded random order. One actual request
runs first to check access, followed by two concurrent workers. There is no
fixed request spacing, no tools, no provider retries, and no answer repairs.
A failure stops new work and is retained; incomplete schedules are not ranked.

Aggregation reproduces production: `(most - least) / comparisons`, average
ranks for ties, integer-rounded percentile basis points, and the same five
display levels. Checks also compare prefixes of two and four rounds without
making additional requests.

Measurements:

- Spearman correlation: Pearson correlation of unrounded average ranks,
  including ties; undefined for an all-tied ranking rather than reported as 1.
- Top/bottom 20% overlap: use 59 slots (`ceil(292 * 0.2)`). If a score tie
  straddles the cutoff, distribute its remaining membership equally across all
  tied passages. Overlap is the sum of minimum membership weights divided by
  59. No passage-ID tie-breaking manufactures an exact top set.
- UI agreement: actual display levels and binary emphasis thresholds at
  levels 2, 3, 4, and 5. A level-5 set can contain more or fewer than 59 passages
  because production keeps tied scores together.
- Passage review: largest percentile changes and consistently high/low
  passages, with full text and all three results.

Stability is not semantic accuracy. Three schedules on one lecture are a small
descriptive experiment, not an independent human evaluation or a guarantee
about other models, providers, or lectures. The existing passage boundaries
include unfinished fragments, which may make broad high/low separation easier.

## Results (2026-09-06)

All **111 requests and 1,752 quartet judgments were valid on their first
attempt**. All three schedules completed in **185.17 seconds (3 minutes
5 seconds)**, including the first serial request. Median request latency was
3.23 seconds, p95 3.69 seconds, maximum 5.89 seconds. There were no provider
errors, retries, or answer repairs. Reported usage was 655,332 prompt tokens
and 24,733 completion tokens.

### More comparisons improved broad agreement

Each row compares the three independent schedules after the indicated number
of comparisons per passage. The means summarize three pairwise comparisons;
those pairs share runs and are not three independent statistical samples.

| Comparisons per passage | Mean Spearman correlation | Pairwise correlation range | Mean tie-aware top-20% overlap |
| ---: | ---: | --- | ---: |
| 2 | 0.680 | 0.655–0.711 | 59.2% |
| 4 | 0.790 | 0.753–0.814 | 65.0% |
| 8 | 0.882 | 0.862–0.895 | 75.1% |

Thus aggregation substantially improves repeatability compared with sparse
comparisons, but does not make the result invariant to a new schedule.

### Eight-round results

| Runs compared | Spearman | Tie-aware top-20% overlap | Exactly same display level | Within one display level |
| --- | ---: | ---: | ---: | ---: |
| 1 / 2 | 0.887 | 78.0% | 160/292 (54.8%) | 280/292 (95.9%) |
| 1 / 3 | 0.895 | 72.9% | 170/292 (58.2%) | 279/292 (95.5%) |
| 2 / 3 | 0.862 | 74.5% | 164/292 (56.2%) | 275/292 (94.2%) |

Median pairwise percentile movement was 7.74–8.08 percentage points. No passage
switched between levels 1 and 5, but the largest movement was **54.46 percentile
points**. Across all three runs, only 115/292 passages (39.4%) retained exactly
the same display level; 259/292 (88.7%) stayed within a one-level range.

For the current binary emphasis UI, actual threshold crossings matter more
than exact-level agreement:

| Emphasis rule | Pairwise passages switching emphasis | Passages switching in at least one of the three comparisons |
| --- | --- | ---: |
| Level ≥4 | 39–46/292 (13.4–15.8%) | 65/292 (22.3%) |
| Level ≥5 | 23–32/292 (7.9–11.0%) | 43/292 (14.7%) |

The level-5 sets contained 50, 59, and 62 passages, with **36 passages in all
three sets**. Forty passages remained level 1 in all three runs. These varying
set sizes reflect tied scores and production percentile bands, not missing
results.

### Inspecting actual passages

Some consistent extremes make pedagogical sense:

- Passage **175**, explaining how associated types are fixed by a trait
  implementation, was selected most in **all 24 comparisons** across the three
  schedules. Its percentile stayed around 99.
- Passage **24**, defining generic type parameters, stayed at percentiles
  90.21, 96.22, and 95.88; passage **250**, explaining the owner/reference
  lifetime invariant, stayed at 95.53, 96.22, and 90.38.
- Passage **291**, dismissing the class, was selected least in **all 24
  comparisons**. Break announcements and several unfinished transitions also
  consistently ranked low.

The unstable cases are not all trivial threshold noise:

- Passage **196**, defining what counts as belonging to one's own crate but
  drifting into unfinished terminology discussion, scored at percentiles
  **50.86, 12.89, and 67.35** (levels **3, 1, 4**). The passage mixes a substantive
  clarification with an aside and cuts off mid-explanation; different quartets
  can emphasize different aspects. This is a plausible interpretation, not a
  proven causal explanation for the movement.
- Passage **35**, a const-generic syntax example, moved through percentiles
  **18.38, 40.55, and 67.35** (levels **1, 3, 4**).
- Passage **14**, connecting language abstractions to the motivation for
  generics, moved through **26.29, 66.84, and 19.59** (levels **2, 4, 1**). Its
  consistent preference over passage 9 in the earlier fixed quartet did not
  imply a stable position against the whole lecture.
- Passage **124**, explaining why deriving a trait depends on field types,
  moved from levels **3, 3, 5**. This is meaningful technical content; changes
  in its visibility deserve attention rather than dismissal as harmless noise.

Stability still does not establish quality. Passage **169**, the unfinished
associated-types introduction flagged in the previous experiment, remained
level 5 in all three runs. Its topic is central, but the passage itself is less
complete than the substantive explanation in passage 175. A stable high rank
does not resolve that concern. Likewise, this experiment does not validate the
technical correctness of statements in the restored transcript.

A post-hoc position check found A selected most in 185/584, 175/584, and
176/584 quartets (30.0–31.7%). With equally many A/B/C/D slots, this suggests
residual first-position sensitivity is worth investigating. However, texts and
opponents differed between positions; this is not a matched-content causal
test and should not be presented as a measured bias coefficient.

## Verdict and next decisions

**Eight comparisons per passage produce a useful broad ranking signal, but
not a reliably repeatable fine-grained ordering or emphasis pattern.** More
comparisons helped considerably in this sample; the strongest core concepts
and low-content passages often separated sensibly. That supports continued
use as a browsing aid, not treating each display level as an authoritative
judgment.

The next production changes can remain narrow: inline A/B/C/D formatting and
provider-configurable request scheduling. This experiment does not justify
automatically increasing global comparison counts, choosing an optimal
concurrency, or changing the importance rubric. A small run at concurrency two
without throttling is not a universal provider limit measurement.

For a subsequent ranking-quality experiment, prioritize position-balanced
comparisons and/or additional independent evidence near important UI
thresholds. Evaluate passage-boundary quality as well. Do not claim that
pooling overlapping schedules proves stability, and do not use caching to
disguise ranking uncertainty. Novelty still needs its own slide-grounded
evaluation.

## Reproduction and artifacts

```bash
uv run scripts/probe_ranking_stability.py \
  run/real_course/glm-5-comparative-analysis-restoration-contract \
  run/real_course/importance-ranking-stability
```

Uses `BEYOND_SLIDES_API_KEY`, never writes it or authorization headers, and
reuses completed valid responses. Use a new output directory for a new live
run. `--prepare-only` and `--summarize-only` do not call the provider.

The output directory contains the frozen `fixture.json`, `manifest.json`, and
`schedules.json`; individual request/response bodies with elapsed times and
validation; `invocation-*.json` with wall time and failures; `summary.json`;
`rankings.json` for two, four, and eight rounds; `passage-stability.json`; and
`passage-review.md` with full texts and all three ranks.

Offline checks:

```bash
uv run --with 'httpx>=0.28,<0.29' python -m unittest discover \
  -s scripts/tests -p 'test_probe_ranking_stability.py' -v
```

All eight tests passed: source shuffle parity, schedule coverage and distinct
round seeds, label mapping, score/percentile parity, invalid-result rejection,
tie-aware cutoff handling, all-tied rankings, and reversed rankings. All 111
saved responses were replay-validated locally and checked for accidental
credential persistence. No Rust code changed, so Cargo checks were not rerun.
