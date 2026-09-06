# Adaptive request scheduling experiment (2026-09-06)

## Decision

Integrate a bounded, feedback-driven attempt scheduler: start at two requests,
grow slowly to a configurable hard ceiling (default eight), share provider
cooldowns, and learn positive request spacing after HTTP 429. Keep fixed mode.
This chooses completion reliability and controlled load over always taking the
fastest setting in one healthy proxy test. It does not discover a universal
maximum or implement provider-specific RPM/TPM accounting.

The [primary-source research](../research/adaptive-request-scheduling.md)
motivates AIMD-style feedback and separate concurrency/rate controls. Raw latency
is not used as proof of congestion: LLM request sizes and output lengths differ.

## Isolated live experiment

Replayed 24 saved production-format requests (12 importance, 12 novelty), sampled
across the completed real lecture. Candidate labels and pre-fetched slide
evidence were unchanged. Each condition used the same shuffled requests twice,
with condition order reversed on repeat. GLM-5, thinking disabled, temperature
zero, 16,384 maximum output tokens, JSON-object output. No repairs or retries
hid model/provider failures. The live probe stops dispatching on a provider
failure and never exceeds eight requests in flight.

| Scheduling | First 24 requests | Repeat | Mean | Valid responses |
| --- | ---: | ---: | ---: | ---: |
| Fixed 2, no spacing | 34.10 s | 36.11 s | 35.10 s | 48/48 |
| Fixed 4, no spacing | 17.78 s | 17.01 s | 17.39 s | 48/48 |
| Fixed 8, no spacing | 10.44 s | 11.21 s | 10.83 s | 48/48 |
| Adaptive 2→8 | 24.43 s | 24.35 s | 24.39 s | 48/48 |

All **192 requests** returned HTTP 200 and valid choices. The eight cells took
**175.42 seconds** in total (sum of measured cell wall times). Adaptive reached
cap five in each short cell; it did not have enough work to reach eight.
Median per-request latency stayed between 2.41 and 2.91 seconds across cells.
Fixed eight was approximately 3.24× faster than fixed two here. This is evidence
that the old cap left capacity unused, not proof that eight is optimal or always
safe. No live throttling was observed, so recovery requires separate tests.

## Deterministic synthetic providers

The discrete-event simulator runs 160 jobs per scenario and three seeds. It
includes a bounded conversation pool, concurrency limits, rolling request-rate limits, heterogeneous latency,
a mid-run capacity drop, and isolated transient 503s. All strategies allow at
most five retries per job. Baseline fixed modes model the previous independent
retry behavior; adaptive modes share cooldowns. Thus this compares complete
strategies, not a controlled isolation of each individual mechanism.

Mean elapsed seconds and mean 429 counts across the three seeds:

| Provider scenario | Concurrency-only AIMD | AIMD + spacing | 429s: concurrency only / combined |
| --- | ---: | ---: | ---: |
| Healthy, unlimited | 28.53 | 28.53 | 0 / 0 |
| Maximum four concurrent | 60.15 | 67.64 | 5 / 3 |
| Three starts per second | 120.35 | 113.73 | 11 / 3 |
| One start/sec, 0.2 s service time | 145.15 | 218.45 | 139 / 7 |
| Heterogeneous 0.25–15 s service time | 172.92 | 172.92 | 0 / 0 |
| Capacity drops from eight to two | 102.57 | 111.25 | 8.3 / 4.3 |
| Isolated transient 503s | 30.81 | 30.81 | 0 / 0 |

Combined adaptation completed every job in all scenarios. Concurrency-only
AIMD exhausted retries on 21 jobs per seed under the one-request/sec quota.
Fixed eight exhausted many retry budgets under restrictive providers; its short
elapsed time there reflects failed work and is not a throughput win. Combined
adaptation pays a conservative recovery cost in some scenarios. These synthetic
limits are explicitly chosen test cases, not fitted estimates of the proxy.
Fixed two completed that quota scenario faster (167.15 s) but made 317
throttled attempts per seed versus seven for the combined strategy. Low retry
pressure is a deliberate part of the selection criterion, not an assertion
that combined adaptation always minimizes elapsed time.

The first simulator version admitted an unbounded pool of conversations waiting
to retry. It overstated the baseline's retry pressure relative to production's
bounded work pool. The corrected table above uses two/eight conversation slots
for the corresponding fixed strategies and eight for adaptive strategies. A
backoff retains a conversation slot while releasing the separate HTTP slot.
The corrected results still support separate rate control. Original simulation
outputs are retained as exploratory artifacts rather than overwritten.

## Production integration check

Ran the real Rust `analyze` command in the separate directory
`run/real_course/adaptive-pipeline-NTRkf5/`, copying only upstream manifests,
traces, and validated restoration/preparation checkpoints from the completed
lecture. The original report and its checkpoints were left untouched. All 296
copied upstream window files still match the original SHA-256 inventory.

The adaptive run recomputed all 110 comparison batches (292 passages), then
rendered the report with 80 slides and token-level audio timing for all passages.
It took **82.57 seconds** including local preparation and rendering; the ranking
timer displayed **1 minute 16 seconds**. The previous fixed-two resumed run took
159.90 seconds overall and displayed 2 minutes 33 seconds for ranking. This is
approximately 1.94× faster overall in these two runs, not a controlled guarantee
across provider load or model response lengths.

The scheduler reached its ceiling of eight with zero 429s and zero provider
failures. There were 111 HTTP responses: 38 importance requests (one structured
answer repair) and 73 novelty requests. Every batch ultimately passed the normal
domain validation. The live telemetry snapshot is preserved as
`request-scheduling-live.json`; its 174,245 ms admission wait is summed across
concurrent requests, not 174 seconds of extra wall time.

Chromium checks passed for all slide images, passage-to-slide and reverse
navigation, hover highlighting, threshold controls, both playback modes,
audio-current passage projection, and the draggable minimap prototype. The
browser reported no page-level JavaScript errors. Artifacts are saved beside
the new `report.html`.

A subsequent offline resume changed mode to fixed, ceiling to three, and the
spacing floor to 25 ms, using an unusable API key and unreachable HTTPS proxy.
All upstream windows and all 110 comparison batches were reused. No model trace
records were added; the current-invocation telemetry reports zero requests.
This checks that operational adaptation remains separate from semantic
checkpoint identity. Full Rust tests, Clippy, formatting checks, and the Python
experiment tests passed before the integration commit.

An initial combined candidate recovered pacing before considering concurrency;
that left an unnecessarily low concurrency cap in place. The selected policy
recovers whichever gate constrained demand. It never raises both controls in
one healthy epoch.

## Selected policy and integration requirements

- Begin at `min(2, ceiling)` and the user's interval floor (normally zero).
- Every actual HTTP attempt uses the gate: initial requests, tools, repairs,
  and retries. Waiting, local tools, and retry backoff hold no HTTP permit.
- After at least `max(8, 2 * cap)` current-generation successes and one healthy
  second, increase cap by one if pending work was concurrency-limited; otherwise
  relax learned pacing by 20% when pacing constrained work. The latency moving
  average only estimates which gate constrains throughput, not congestion.
- On 429, halve the cap (minimum one) and raise spacing to the maximum of twice
  its previous value, successful-response latency divided by the reduced cap,
  and 250 ms. Respect the user's interval floor. A cohort/generation identifier
  permits only one decrease per already-dispatched throttled wave.
- Every 429 can extend the shared cooldown, including late responses. Old
  successes cannot undo a reduction. Retry-After supports seconds and HTTP
  dates; valid long delays must not be shortened. Missing advice uses bounded
  backoff. Cancellation releases capacity without counting as success.
- Other transient provider errors retain bounded retries and suppress immediate
  growth; a 5xx is not automatically treated as evidence of an RPM quota.
- Share the scheduler through the CLI's sequential stages. Exclude policy from
  semantic checkpoint identity and record operational telemetry separately.

The Python experiment excludes async cancellation mechanics and uses simplified
numeric Retry-After values; Rust tests must cover those integration concerns.
Long quotas, exhausted credit, external account traffic, multi-process
coordination, and token-aware budgets are not solved by this first controller.

## Artifacts and reproduction

```bash
uv run scripts/probe_request_scheduling.py run/real_course/new-scheduling-probe \
  --live-trace run/real_course/glm-5-comparative-analysis-restoration-contract/model-trace.jsonl
```

Uses `BEYOND_SLIDES_API_KEY`; `--api-key-stdin` is also available. Omit
`--live-trace` to run only simulations. A new output directory is required so
timing measurements do not silently mix fresh and cached responses. The script
is restricted to the existing GLM-5 comparison payload for this live experiment;
the production scheduler itself must remain provider-neutral.

Live results are in `run/real_course/scheduling-experiment/`: `simulation.json` (initial unbounded-pool simulation),
`sample.json`, `live-summary.json`, and each cell's complete responses. Earlier
simulation iterations remain separately under `scheduling-simulation-v1/` and
`scheduling-simulation-v2/`; corrected bounded-pool results are in
`scheduling-bounded-pool-simulation/simulation.json`. No production checkpoints or reports were modified
by this experiment, and no credentials were written into its artifacts.
