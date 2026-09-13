# Running the comparative pipeline

`analyze` restores the transcript, prepares source-backed semantic passages,
ranks importance and novelty separately, and renders the continuous report.
The comparisons use inline A/B/C/D candidates. Saved results contain canonical
passage IDs, not labels. Novelty receives pre-fetched slide evidence; comparison
requests do not use tools.

## Provider and scheduling

Set `BEYOND_SLIDES_API_BASE_URL`, `BEYOND_SLIDES_MODEL`, and
`BEYOND_SLIDES_API_KEY` for your OpenAI-compatible endpoint. Do not put keys in
source files or commit them. To enter a key without placing it in shell history:

```bash
read -r -s -p 'API key: ' BEYOND_SLIDES_API_KEY
export BEYOND_SLIDES_API_KEY
```

Optional scheduling settings apply to restoration, passage preparation, and
comparative ranking:

```bash
export BEYOND_SLIDES_SCHEDULING=adaptive
export BEYOND_SLIDES_INITIAL_CONCURRENCY=2
export BEYOND_SLIDES_MAX_CONCURRENCY=8
export BEYOND_SLIDES_REQUEST_INTERVAL_MS=0
```

These are the defaults. Adaptive mode starts with the configured initial
concurrency and can grow to the configured ceiling. The interval is a **floor**, not a promise
that requests will always start that quickly: HTTP 429 can lower concurrency,
introduce spacing, and establish a shared cooldown. Healthy queued work lets
the scheduler recover gradually. It does not probe above the ceiling, cancel
already-running requests when the cap falls, or infer congestion from latency
alone. Pending work enters this scheduler directly; no request runs alone as a
mandatory preflight.

Initial and maximum concurrency must be positive, and the initial value cannot
exceed the ceiling; the interval must be a nonnegative integer in milliseconds.
Initial calls, tool follow-ups, repairs, and retries share the same gate. The
CLI shares learned state across its sequential stages, but not across unrelated
runs. Successful checkpoint reuse is unaffected by scheduling. Your shell's
existing `BEYOND_SLIDES_MAX_CONCURRENCY=2` remains a hard ceiling unless you
change or unset it.

To stop a run before it starts further model requests after a recorded usage
limit is reached, set an optional positive integer budget:

```bash
export BEYOND_SLIDES_TOKEN_BUDGET=1000000
```

The budget is the sum of total input tokens (including the cached subset once)
and output tokens reported by completed responses across restoration and
analysis. Existing traces are counted when a run resumes. Requests already in
flight may finish slightly beyond the limit; subsequent requests are blocked
and the run remains resumable. If a provider omits input or output usage while
a budget is active, BeyondSlides pauses rather than pretending the missing
usage was zero. Raise the value or unset it to continue from validated
checkpoints.

Slow-tail hedging is automatic and intentionally conservative. Latency is
tracked separately for every workflow/request-kind class. Nothing is hedged
until that class has ten successful responses, so a model that normally takes
five minutes is not mistaken for a stalled model during warm-up. Thereafter,
one identical backup attempt may start when the original exceeds
`3 × rolling p80 + 5 seconds`. The first successful provider response wins;
the other attempt is cancelled. At most three backup attempts may be active at
once and at most twenty may start during one resumable run. These exceptional
attempts can temporarily exceed ordinary concurrency by three so they do not
wait behind the same busy queue. Existing model traces restore the latency
samples and consumed hedge budget when a run resumes.

For predictable manual limits:

```bash
export BEYOND_SLIDES_SCHEDULING=fixed
export BEYOND_SLIDES_MAX_CONCURRENCY=4
export BEYOND_SLIDES_REQUEST_INTERVAL_MS=0
```

Fixed mode defaults to concurrency two if its ceiling is unset; it does not
learn concurrency or spacing, but still respects shared provider cooldowns.
Adaptive mode is a conservative portable policy, not a guarantee of maximum
throughput. It has no provider-specific token quota accounting or knowledge of
other users' traffic. See the [experiment](evaluation/adaptive-request-scheduling.md).

Transient provider failures retain bounded retries: two retries per restoration
request, five per passage-preparation or comparison request. Invalid structured
responses first allow two in-conversation repair attempts. Invalid transcript
restorations, restored-annotation text-integrity failures, and invalid importance
or novelty comparison batches then allow up to five clean conversations. These
are separate budgets, not unlimited retries. A failed run retains its completed,
validated checkpoints.
Retry-After accepts seconds or HTTP dates; long valid advice is not clipped.
Retry jitter helps stagger independent clients, and absent advice uses bounded
backoff.

The progress message shows the effective HTTP cap/ceiling, current spacing,
and consumed hedge budget after each completed work item. The web interface
also reports hedges separately from provider retries. `request-scheduling.json`
records a cumulative snapshot for this invocation at checkpoints and stage
completion/interruption: effective settings, peak in-flight requests, HTTP
successes/failures/429s, cancellations, hedges, and total admission wait. HTTP
success is not semantic validation. Admission wait is summed across concurrent
requests, not additional wall time. The main run's final snapshot includes
restoration when that stage made requests. Snapshots are overwritten on resume;
model traces retain individual attempts and restore the hedging history.

Provider-specific fields remain optional. For the current GLM-5 proxy run:

```bash
export BEYOND_SLIDES_API_BASE_URL='https://lab.cs.tsinghua.edu.cn/ai-platform/api/v1'
export BEYOND_SLIDES_MODEL='glm-5'
export BEYOND_SLIDES_CHAT_EXTRA_BODY='{"thinking":{"type":"disabled"}}'
```

The stage-specific `BEYOND_SLIDES_RESTORATION_CHAT_EXTRA_BODY` and
`BEYOND_SLIDES_ANNOTATION_CHAT_EXTRA_BODY` override the common value if set.
Keep them consistent with the run being resumed. Do not assume this thinking
field is supported by other providers.

## Real lecture command

```bash
cargo run --release -- analyze \
  run/real_course/transcript.json \
  run/real_course/slides.json \
  run/real_course/glm-5-comparative-analysis-restoration-contract \
  --slides-pdf data/real_course/slides.pdf \
  --audio run/real_course/audio.flac \
  --timed-tokens run/real_course/timed-tokens.json
```

This produces `analysis.json`, `report.html`, and `report.assets/` in the run
directory. The assets contain slide images and the audio; keep them alongside
the HTML when sharing it. The timing sidecar projects passage playback onto
word/token timestamps when a reliable local match exists, with coarse source
segment timing as a fallback.

## Resuming safely

### Passage segmentation

Passage preparation always classifies candidate boundaries across the restored
transcript and selects the final source-exact partition in Rust:

```bash
cargo run --release -- analyze \
  run/real_course/transcript.json \
  run/real_course/slides.json \
  run/real_course/passage-quality-20260906/production \
  --slides-pdf data/real_course/slides.pdf \
  --audio run/real_course/audio.flac \
  --timed-tokens run/real_course/timed-tokens.json
```

Use the provider/thinking settings that match the restored checkpoints. The
pipeline restores raw transcript windows as before, then assigns a `0..5` cut
cost to candidate gaps across the whole restored text. It does **not** run the legacy
tool-calling passage preparation. New passages get inferred slide positions,
then both comparative metrics run with pre-fetched novelty evidence. Related
slides, summaries, and notes are not judged in this mode; their fields are
empty, not transferred from old passages.

Boundary checkpoints are stored under `boundaries/<identity-hash>/`. Changing
the boundary prompt or task inputs selects a different directory; scheduling
and comparison changes reuse completed classifications. Every loaded checkpoint
is revalidated. Changing segmentation reruns the relevant comparisons but not
restoration. The source hashes still guard against adopting another lecture.
Completed `analysis.json` artifacts from the former window-owned preparation
path remain renderable. New and resumed analysis execution does not expose or
select that legacy path.

Rust selects a source-exact partition with a 300-character maximum. Every gap
remains cuttable: the dynamic program first avoids the most damaging cuts, then
minimizes passage count and balances otherwise equivalent passage lengths.
Long punctuation-free text receives UTF-8-safe fallback candidates, so semantic
judgments cannot make the partition infeasible.

`boundary-preparation.json` contains segmentation diagnostics. The legacy
`window_diagnostics` and `window_projections` arrays are empty in its analysis
artifact because there was no copied-text projection stage. A pre-existing
`annotation-quality.json` describes the old window mode, not the new run.

### Checkpoint compatibility

Run the same command against the same directory. Completed restoration windows,
passage-preparation windows, and comparison batches are validated and reused.

Mutating `analyze` and `restore` runs hold an exclusive OS
lock in `.run.lock` until exit. A second writer is rejected before checkpoint
mutation. The lock file remains after exit; do not delete it to bypass an active
run. The OS releases the lock if the process exits or crashes. Analysis also
locks its nested restoration stage while that stage runs. Older binaries do
not honor this new lock: never mix an older active writer with a new one.

- Changing scheduling mode, concurrency, or pacing does not redo completed model work.
- Changing an importance or novelty prompt does not invalidate restoration or
  passage preparation. Only that metric gets a new comparison namespace.
- Changes to comparison grouping or novelty evidence settings invalidate the
  affected comparison checkpoints, not upstream stages.
- Changed sources, restoration/preparation prompts, models, or relevant semantic
  configuration still require a new run directory. Checkpoint corruption and
  invalid domain results remain errors, even with matching manifests.

Comparison checkpoints live under
`comparisons/<metric>-<configuration-hash>/`, with a manifest binding them to
their exact prepared passages and configuration. Old namespaces are preserved;
old flat comparison checkpoints are not silently adopted. The root's legacy
manifest records its initial configuration and is left unchanged on resume.
`execution-settings.json` in the root and restoration directories records the
latest scheduling settings, not a replacement source/provenance manifest.

The transcript may still be windowed and scored locally to validate and rebuild
tasks on resume. Reusing completed model results does not mean skipping those
deterministic checks.

## Model backends

The default backend is an OpenAI-compatible endpoint configured with
`BEYOND_SLIDES_API_BASE_URL`, `BEYOND_SLIDES_API_KEY`, and
`BEYOND_SLIDES_MODEL`.

To use a ChatGPT subscription through the local Codex CLI, first run
`codex login`, then set:

```sh
export BEYOND_SLIDES_MODEL_BACKEND=codex
export BEYOND_SLIDES_MODEL=gpt-5.6-luna
# Optional per-turn controls advertised by the selected model:
export BEYOND_SLIDES_CODEX_REASONING_EFFORT=medium
export BEYOND_SLIDES_CODEX_SERVICE_TIER=priority
```

The Codex backend does not read the API URL, API key, or provider-specific
extra-body variables. It starts one local app-server lazily, uses ephemeral
read-only task threads in an empty directory, and retains BeyondSlides'
checkpoint, progress, token-usage, validation, and model-trace behavior.
The web application exposes the same choice under “模型连接方式”, reads the
authenticated account's live model catalog, and limits reasoning-effort and
speed choices to the capabilities advertised for the selected model. The
service-tier value is operational and does not invalidate checkpoints;
reasoning effort is part of model-result identity and does.
