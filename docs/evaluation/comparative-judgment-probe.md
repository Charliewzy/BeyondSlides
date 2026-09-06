# GLM-5 comparative importance: batch size and candidate order

Run on 2026-09-06 against the Tsinghua proxy, using `glm-5`, thinking disabled,
temperature 0, JSON-object output, and a 16,384-token output cap. This is an
importance-only diagnostic experiment, not a novelty or retrieval evaluation.
No production Rust code, prompts, or checkpoints were changed for this test.

## Design

Source: the 292 prepared passages in
`run/real_course/glm-5-comparative-analysis-restoration-contract/`.
The sample contains 16 quartets:

- Four original quartets, including all three that previously produced
  out-of-group selections in the failed importance batch.
- Six intentionally clear contrasts.
- Six difficult comparisons involving competing core concepts, practical
  advice, or closely related explanations.

Passage text was kept verbatim. The source run still contains unfinished
fragments; this experiment does not silently fix them. Expectations and review
notes were saved before any model call and were not shown to the model.
They are Codex's provisional judgments, **not independent human gold labels**.
No unique answer was invented for the difficult quartets.

Crossed conditions:

- Existing separate global-ID lists and passage pool, versus inline text with
  A/B/C/D labels mapped back to global IDs by the script.
- Four versus sixteen quartets per request.
- Original candidate order, an identical-input repeat, reversed order, and a
  seeded shuffled order. Group order stays fixed. In the global-ID format,
  permutations change each group's ID list while the separate passage pool
  remains sorted, matching production behavior. In the inline format, the
  actual candidate texts move. Thus order-sensitivity percentages should not
  be treated as a perfectly controlled comparison *between formats*.

The importance rubric was otherwise unchanged. Requests were interleaved in a
seeded random schedule to reduce server-time confounding. One actual
experimental request ran first to check access, then concurrency was two.
There were no provider retries, tools, or final-answer repairs.

## Timing and structural validity

All **40 requests completed in 38.28 seconds wall-clock**. They requested 256
quartet judgments (repeated measurements of 16 quartets), of which 253 were
valid. Reported usage totaled 122,116 prompt tokens and 3,782 completion tokens.

| Input format | Quartets/request | Accepted requests | Valid judgments | Median request time |
| --- | ---: | ---: | ---: | ---: |
| Global IDs | 4 | 16/16 | 64/64 | 1.42 s |
| Global IDs | 16 | 3/4 | 61/64 | 3.07 s |
| Inline A/B/C/D | 4 | 16/16 | 64/64 | 1.41 s |
| Inline A/B/C/D | 16 | 4/4 | 64/64 | 3.07 s |

The failing global-ID request selected passages from other groups in three
comparisons. For example, comparison 2 contained `[222, 188, 38, 288]`, but the
model selected `most=235, least=231`. A/B/C/D eliminated this observed failure
in this sample; 128 valid selections do not prove it can never fail.

## Consistency

Agreement below requires **both most and least to match**, after translating
labels back to passage IDs. Denominators include only valid judgments in both
conditions; invalid judgments are reported separately above.

| Input format | Quartets/request | Identical repeat | Reversed order | Shuffled order |
| --- | ---: | ---: | ---: | ---: |
| Global IDs | 4 | 13/16 | 11/16 | 14/16 |
| Global IDs | 16 | 11/16 | 6/13 | 5/16 |
| Inline A/B/C/D | 4 | 14/16 | 11/16 | 7/16 |
| Inline A/B/C/D | 16 | 15/16 | 11/16 | 8/16 |

Inline choices were relatively repeatable with identical input, but candidate
order still mattered. Reducing the batch to four did **not** clearly improve
this behavior. The two inline batch sizes agreed on 13/16 baseline pairs.

Disagreements were concentrated in harder material. For inline four-group
batches, clear-case pairs stayed the same in 6/6 reversals and 5/6 shuffles;
difficult-case pairs stayed the same in only 2/6 reversals and 1/6 shuffles.
For inline sixteen-group batches the corresponding counts were 6/6 and 5/6
for clear cases, versus 1/6 and 3/6 for difficult cases.

These are small, deliberately selected, correlated samples. They do not
estimate lecture-wide accuracy or prove a general optimal batch size.

## Reviewing actual choices

### Strong, defensible contrasts

- **Generic type parameters versus a personal aside about mastering C++**
  (comparison 4, passages 24 and 22): every condition chose the definition as
  most and the aside as least.
- **Reference/owner lifetime validity versus class dismissal** (comparison 5,
  passages 250 and 291): every inline condition chose the core invariant as
  most and dismissal as least. The global-ID sixteen-group baseline instead
  preferred the apostrophe syntax explanation, passage 253. I consider that a
  weaker pedagogical choice, not a formatting failure.
- **Compile-time requirements versus an unfinished transition** (comparison
  7, passages 36 and 10): inline choices were stable and sensible. Global-ID
  sixteen-group baseline/repeat instead marked a partial technical explanation
  as least while retaining the largely content-free transition.

### My initial label was too confident

In comparison 8 I expected passage 9 (copy-paste creates synchronized
maintenance work) to outrank passage 14 (connect language abstractions to the
motivation for generics). Both inline batch sizes consistently preferred 14.
That is defensible under the course-centrality rubric: it connects directly to
this lecture's subject. It is not fair to automatically score those responses
as model errors.

Keeping the pre-run labels unchanged, exact agreement on the six nominally
clear cases was 19/24 for **each** inline batch size, 20/24 for global IDs with
four groups, and 13/23 valid judgments with sixteen groups. Four disagreements
in each inline condition came from comparison 8. These are label-agreement
counts, not an objective accuracy percentage. The remaining inline mismatch
was choosing a mechanical constructor example, rather than an isolated
"can match this" fragment, as least valuable in comparison 6.

### Hard choices and remaining concerns

- **Four core mechanisms** (comparison 10): generic types, trait bounds,
  static dispatch, and lifetime safety all carry substantial learning value.
  With inline four-group batches, the lifetime-safety passage moved from least
  in the baseline to most after reversal. A forced best--worst choice among
  these is unstable; neither endpoint should be interpreted as an absolute
  judgment that the passage is unimportant.
- **Same-topic explanations** (comparison 13): inline requests consistently
  selected the compile-time rationale as most. The least choice moved among
  syntax, a restriction, and a partially redundant explanation. That looks
  more like a close call than a fundamental comprehension failure.
- **An introduction outranking its explanation** (comparison 1): baseline and
  repeat inline results preferred passage 169's unfinished introduction to
  associated types over passage 245's explanation of dynamic-compatibility
  constraints and passage 124's derivation requirements. I find that
  questionable: naming a central topic is not the same as teaching it. The
  shuffled choices changed again. This is an example where valid, repeated
  output alone does not establish quality.
- **A confusing self-correction** (comparison 14): every inline condition
  marked passage 261 as least. The most choice varied between two meaningful
  lifetime explanations, a defensible ambiguity.

## Verdict

1. Adopt the inline A/B/C/D input/output package for its simpler bookkeeping;
   do not claim that it solves semantic judgment reliability.
2. This sample provides **no clear quality case for reducing inline importance
   batches from sixteen to four**. Four batches also require more requests;
   median per-request time alone is not full-stage throughput.
3. Treat individual best--worst choices as noisy observations. Vary and ideally
   balance candidate positions over repeated comparisons, and examine the
   stability of the *aggregated* ranking before trusting fine distinctions.
   This experiment did not measure aggregate rank stability.
4. Improve or separately evaluate the unfinished passage boundaries. Several
   apparent easy wins are a concept-versus-fragment comparison, not evidence
   that nuanced lecture ranking is solved.
5. Evaluate novelty separately with fixed, inspected slide evidence. Nothing
   here establishes novelty quality, tool requirements, or thinking-mode
   performance.

## Reproduction and artifacts

Experiment script: `scripts/probe_comparative_judgments.py`.

```bash
uv run scripts/probe_comparative_judgments.py \
  run/real_course/glm-5-comparative-analysis-restoration-contract \
  run/real_course/comparative-judgment-probe
```

The script uses `BEYOND_SLIDES_API_KEY`; endpoint and model come from the source
run's saved request. Existing responses are reused. Use a new output directory
to repeat live inference. The fixture is intentionally bound to this particular
292-passage run and verifies passage text against the original request.

Under the output directory:

- `fixture.json`: exact texts and pre-run assessments.
- `manifest.json`: fingerprint and experimental settings.
- `*.request.json`, `*.response.json`: complete request bodies, provider
  responses, per-request elapsed times, and structural validation. No API key
  or authorization headers are saved.
- `summary.json`: counts and mapped choices, including category-specific
  agreement.
- `choice-review.md`: all sixteen quartets with full text and every decision
  side by side.
- `invocation-*.json`: measured wall time and concurrency.

Offline harness checks verified all forty label mappings, rejection of invalid
selections, identical baseline/repeat payloads, and exclusion of reviewer
labels from model inputs. No Rust files were changed for this experiment, so
Cargo checks were not rerun.
