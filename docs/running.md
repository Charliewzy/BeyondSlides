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
export BEYOND_SLIDES_MAX_CONCURRENCY=8
export BEYOND_SLIDES_REQUEST_INTERVAL_MS=0
```

These are the defaults. Adaptive mode starts with two simultaneous requests and
can grow to the configured ceiling. The interval is a **floor**, not a promise
that requests will always start that quickly: HTTP 429 can lower concurrency,
introduce spacing, and establish a shared cooldown. Healthy queued work lets
the scheduler recover gradually. It does not probe above the ceiling, cancel
already-running requests when the cap falls, or infer congestion from latency
alone. The first window and metric canaries still run before later work.

Concurrency must be positive; the interval must be a nonnegative integer in
milliseconds. Initial calls, tool follow-ups, repairs, and retries share the
same gate. The CLI shares learned state across its sequential stages, but not
across processes or later invocations. Successful checkpoint reuse is unaffected
by scheduling. Your shell's existing `BEYOND_SLIDES_MAX_CONCURRENCY=2` remains
a hard ceiling unless you change or unset it.

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
responses allow two repair attempts. These are separate budgets, not unlimited
retries. A failed run retains its completed, validated checkpoints. Retry-After
accepts seconds or HTTP dates; long valid advice is not clipped. Retry jitter
helps stagger independent clients, and absent advice uses bounded backoff.

The progress message shows the effective HTTP cap/ceiling and current spacing
after each completed work item. `request-scheduling.json` records a cumulative
snapshot for this invocation at checkpoints and stage completion/interruption:
effective settings, peak in-flight requests, HTTP successes/failures/429s,
cancellations, and total admission wait. HTTP success is not semantic validation.
Admission wait is summed across concurrent requests, not additional wall time.
The main run's final snapshot includes restoration when that stage made requests.
Snapshots are overwritten on resume; model traces retain individual attempts.

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

Run the same command against the same directory. Completed restoration windows,
passage-preparation windows, and comparison batches are validated and reused.

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
