# BeyondSlides Architecture

Status: Working architecture

## 1. Product goal

BeyondSlides finds the useful information a lecturer contributes orally beyond
the course's written sources. It is not a general lecture summarizer. Given a
timestamped transcript and a slide deck, it identifies explanations,
intuitions, caveats, examples, practical advice, and conceptual connections
that are valuable to a learner but are not already explicit in the slides.

The system preserves two kinds of evidence:

- readable lecture text remains backed by the raw transcript-segment range from
  which it was restored;
- semantic judgments may cite related slides from the immutable slide deck.

Importance and novelty are ranked relative to other passages in the same
lecture through repeated best--worst comparisons. Their stored comparison
counts and percentiles remain uncertain semantic judgments, not objective
facts or cross-lecture measurements. Reports derive `1..5` display levels from
those percentiles. Connection strength remains a per-passage `0..5` judgment.
Optional comparison notes support debugging and evaluation but are neither
required nor shown to a learner by default.

## 2. Current user experience

The end-to-end command accepts normalized `transcript.json` and `slides.json`
and writes a resumable run directory. It first restores readable lecture text,
then analyzes that restored text against the slides.

The primary report preserves lecture order. Importance controls character
weight; novelty controls underline thickness. A reader can therefore follow
the lecture continuously while visually locating high-value oral additions.
Selecting a passage reveals its scores, time range, and raw source range. When
a slide PDF is supplied at rendering time, a scrollable rail displays real
slide pages beside the transcript and centers the selected passage's inferred
slide position. The aligned page retains a blue highlight while the page at the
center of the independently scrolled rail is enlarged.

The completed analysis is also a standalone, validated JSON artifact. Reports
and quality summaries can be regenerated from saved artifacts without another
model request. A legacy raw-transcript annotation renderer remains available
for comparison while the restored-text experience is evaluated.

The initial language target is Simplified Chinese, including Latin technical
terms, formulas, and identifiers commonly mixed into Chinese course material.

## 3. Architectural principles

### Preserve sources

Raw transcript and slide content are immutable source records. Restoration
creates a derived readable representation with explicit provenance; it never
silently replaces the source transcript.

### Prefer deterministic structure around probabilistic judgment

Normalization, window ownership, retrieval, slide-path decoding, projection,
schema validation, checkpoint validation, assembly, and rendering are
deterministic Rust stages. Models restore speech and make semantic judgments.

### Expose the right precision

A lecture passage may begin inside a restored transcript span. Its readable
text boundary is precise, while its raw transcript provenance remains the
coarser source range of the supporting restored spans. Adjacent passages may
therefore cite the same raw range. The system does not invent finer timestamp
or source precision.

### Separate chronological position from semantic relevance

A slide position is an approximate chronological location inferred by dynamic
programming. A related slide is semantic evidence for a judgment. The former
supplies local context and never proves the latter.

### Make expensive stages resumable and observable

Restoration and passage preparation persist validated window checkpoints;
comparative ranking persists validated metric batches. Run manifests bind all
checkpoints to source identities, prompts, models, and deterministic
configuration. Complete model exchanges are recorded without credentials.

### Keep presentation downstream

HTML generation consumes only validated artifacts. Prompt, model, and
retrieval work must not be repeated to change typography or interaction.

## 4. Domain model

The canonical vocabulary is defined in `CONTEXT.md`; ADRs under `docs/adr/`
record resolved decisions.

### Source coordinates

```rust
struct TranscriptSegmentId(u32);
struct SlideId(u32);
```

Both IDs equal their zero-based collection position in one normalized source
snapshot. Human-facing page numbers may be one-based but are not IDs. Changing
normalization invalidates downstream artifacts.

```rust
struct TranscriptSegment {
    id: TranscriptSegmentId,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

struct Slide {
    id: SlideId,
    text: String,
}
```

`ValidatedSources` establishes ID, ordering, timestamp, and source-text
invariants once. Downstream code can use direct indexed lookup without treating
collection indices as unrelated identities.

### Restored transcript

```rust
enum RestoredTranscriptSpan {
    Text {
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
        text: String,
    },
    OmittedDisfluency {
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
    },
}

struct RestoredTranscript {
    spans: Vec<RestoredTranscriptSpan>,
}
```

Restored spans partition the entire raw transcript. Text spans add punctuation,
join speech fragments, and minimally regularize speech. Omitted spans preserve
the provenance of pure disfluencies. Neither form may summarize or add meaning.

### Lecture passages

```rust
struct RestoredLecturePassage {
    text: String,
    source_start: TranscriptSegmentId,
    source_end: TranscriptSegmentId,
    slide_position: SlideId,
    novelty: Score5,
    connection_strength: Score5,
    importance: Score5,
    comparative_novelty: Option<ComparativeScore>,
    comparative_importance: Option<ComparativeScore>,
    related_slides: Vec<SlideId>,
    summary: Option<String>,
    comparison_note: Option<String>,
}
```

Lecture passages partition the readable restored transcript in chronological
order. Their `text` is authoritative restored text, never trusted model copy.
`novelty` and `importance` are coarse display levels derived from their
lecture-wide comparative scores. Comparative evidence stores comparison count,
most and least selections, and percentile in basis points. The optional form
keeps older preliminary artifacts readable; a newly completed analysis has
both comparative scores for every passage. Connection strength remains a
validated inclusive `0..5` value. Related slide IDs must exist and contain no
duplicates. Summary and comparison note remain optional.

## 5. Pipeline

```text
video/audio             slide PDF
    |                        |
    v                        v
transcript adapter       PDF text adapter
    |                        |
    +-----> validated normalized sources <-----+
                         |
                         v
                 raw transcript windows
                         |
                         v
                model restoration stage
                         |
                         v
                  restored transcript
                         |
                         v
                restored-text windows
                         |
              +----------+----------+
              |                     |
              v                     v
       all-slide scoring      slide-path decoder
              |                     |
              +----------+----------+
                         v
       model passage preparation + slide tools
                         |
                         v
             fuzzy boundary projection
                         |
                         v
             validation and assembly
                         |
              +----------+----------+
              |                     |
              v                     v
    importance comparisons   novelty comparisons
                                    |
                           slide evidence sets
              |                     |
              +----------+----------+
                         v
             best--worst aggregation
                         |
                         v
              artifact and HTML report
```

An optional timed-token sidecar follows a separate path from the ASR system to
report rendering. It refines passage audio playback but does not participate in
restoration, annotation, or source provenance.

### 5.1 Ingestion and normalization

The FunASR TSV adapter converts each nonblank timestamped row into one
zero-based transcript segment. It preserves ASR evidence instead of inventing
grammatical boundaries. The PDF adapter uses Poppler `pdftotext` in raw reading
order, preserves one slide per page, removes only rigorously detected repeated
page furniture, and reports sparse text or suspicious glyphs rather than
silently invoking OCR.

For recordings transcribed with SenseVoice, the optional timing exporter
retains each fine-grained ASR text unit and its interval as a timed transcript
token. Chinese tokens are commonly individual characters; consumers must not
assume that tokens are linguistic words. This sidecar supplements rather than
replaces the coarser normalized transcript.

The original video is not required by the product. When available, FFmpeg,
Poppler, and MSSIM can build a visual slide/time reference for evaluating
semantic alignment; that reference is not an input to production alignment.

### 5.2 Restoration windowing

Raw transcript windows own nonoverlapping ranges and may expose left and right
context. Owned regions partition the raw transcript. Character and duration
budgets greedily include complete transcript segments; one oversized segment
remains intact.

The restoration model may only emit spans over the owned region. Validation
requires exact source-range coverage with no gaps, overlap, or context claims.
Accepted window restorations assemble directly in source order.

### 5.3 Restored-text windowing

Annotation uses a second window sequence over restored spans. Character budgets
count readable text; duration comes from each span's raw source range. A
restored span is never split by a window, and omitted-disfluency spans remain in
an adjacent owned region so provenance is not lost. Owned regions together
partition the restored transcript.

### 5.4 Retrieval and slide positions

```rust
trait SlideScorer {
    fn score_slides(&self, query: &str) -> Result<Vec<SlideScore>, SearchError>;
}
```

Every scorer returns one finite score per slide in presentation order:

- lexical scoring uses BM25, Unicode compatibility normalization, and Jieba
  search-mode segmentation;
- dense scoring uses `BAAI/bge-small-zh-v1.5`, applies the model's Chinese query
  instruction, and brute-forces in-memory cosine similarity;
- hybrid scoring combines complete rankings with equal-weight reciprocal-rank
  fusion (`k = 60`).

Dynamic programming consumes one all-slide score row per restored window. It
selects a soft, non-strictly-monotonic slide position path: nearby forward or
backward moves are cheap, and large jumps remain possible with higher cost.

### 5.5 Passage-preparation conversation

The model receives left context, owned readable text, right context, inferred
slide position, and a nearby slide neighborhood. It may call:

```text
inspect_slide(slide_id)
search_slides(query, max_results)
```

Search covers the entire deck so the model can find semantic slide evidence
beyond its inferred position. A per-window tool session suppresses duplicate
slide text after first exposure. Tool rounds are bounded. After the last
permitted tool result, the client removes tool definitions and tool choice from
the next request, forcing a final-answer attempt at the protocol level rather
than trusting the model to count rounds.

The model returns proposed passages containing copied text, connection
strength, and related-slide evidence. It does not evaluate importance or
novelty, so those scores cannot influence its semantic boundaries. It never
supplies trusted offsets or raw source IDs.

### 5.6 Source-backed passage projection

The proposed passage texts are concatenated and aligned to the authoritative
owned restored text with a Unicode-scalar Myers diff. Proposed boundaries are
projected through that alignment onto UTF-8 byte boundaries in the source.

An accepted projection must satisfy all of the following:

- proposed and projected passages are nonempty and ordered;
- projected ranges exactly partition the authoritative owned text;
- changed characters are strictly less than 5% of the larger source/proposed
  character count.

At 5% or above, validation returns the error to the same model conversation for
a bounded final-answer repair. Below 5%, only boundary intent is accepted: the
model's copy is discarded and every passage receives the corresponding exact
source slice. Projection diagnostics record changed and compared character
counts per accepted window.

Each projected byte range is mapped to the first and last supporting restored
span. Those spans supply the passage's coarse raw transcript provenance.

### 5.7 Comparative importance and novelty

After the complete passage partition is assembled, the comparative-ranking
module builds deterministic best--worst groups. The default plan uses groups of
four across eight independently shuffled rounds. Each passage therefore
appears repeatedly against different lecture peers. Small lectures use the
largest possible group of at least two; a one-passage lecture receives a
neutral percentile without a model request.

Importance and novelty are separate conversations:

- importance compares the learning loss if a student omitted each passage;
- novelty compares how much useful, non-obvious content each passage adds
  beyond a supplied set of slide evidence.

Novelty evidence combines the inferred slide-position neighborhood,
related-slide evidence collected during passage preparation, and the highest
hybrid retrieval candidates. Slide text is deduplicated within each request.
Neither metric sees preliminary absolute scores or the other metric's result.
Each group contains its candidate texts inline under local A/B/C/D labels.
The model selects labels; Rust resolves them to canonical passage IDs before
validation, checkpointing, and aggregation. Novelty candidates reference the
same deduplicated slide-text pool as before.

Every response must return each requested comparison exactly once, select two
different labels present in that group, and add no unknown comparisons. One batch for
each metric runs as a canary before remaining batches run with bounded
concurrency. Validated batches are checkpointed independently and can be
resumed.

For each metric, aggregation computes `(most - least) / comparisons`, ranks
that balance across the lecture with average ranks for ties, and stores the
percentile in basis points. Percentile bands produce `1..5` display levels.
Those levels intentionally improve within-lecture visual discrimination; they
must not be interpreted as calibrated absolute scores.

### 5.8 Assembly and persisted artifacts

Per-window checkpoints contain source-backed passages, model diagnostics, and
projection diagnostics. Restoring a checkpoint reruns deterministic
window-local validation before trusting it. Assembly orders window results,
validates each partition again, then validates the complete restored transcript
and lecture-wide passage partition through the same shared partition routine.

`analysis.json` is a `RestoredAnalysisArtifact` containing:

- the complete restored transcript;
- all chronological lecture passages, each carrying its inferred slide
  position and comparative importance/novelty evidence;
- per-window model diagnostics;
- per-window projection diagnostics.

The offline `render-analysis` command revalidates that artifact against the raw
transcript and slide deck before rendering. Metadata arrays must agree on their
window count, slide positions must exist, and passage text/provenance must still
match the restored transcript.

### 5.9 Rendering and evaluation

The continuous report renders authoritative passage text in lecture order.
User-controlled discrete thresholds map importance to bold versus normal text
and novelty to underlined versus plain text; the underlying display levels and
comparative percentiles remain available to report consumers. The fixed
upper-right controls range from
highlighting every score through disabling a channel, and preserve the reader's
viewport anchor when font-weight changes reflow the transcript. Hover, focus, or
click reveals timestamps, raw source range, inferred slide position, and
component scores.

When audio and timed transcript tokens are supplied, rendering locally aligns
each passage's restored text within its coarse source interval and projects its
boundaries onto token timing. Token-derived adjacent passages share one audio
boundary. Passages with overlapping provenance are aligned as one ordered
group, preventing repeated phrases from reversing their playback boundaries. A
group falls back to its coarse source-segment intervals when any passage has
less than a 60% normalized character match. These playback intervals remain
presentation data, so adding or improving the timing sidecar does not
invalidate `analysis.json`.

An optional PDF presentation adapter invokes Poppler once at rendering time and
writes one ordered PNG per normalized slide beneath a report-local asset
directory. The slide count must exactly match the normalized slide deck. The
HTML references only those local images and embeds no remote assets. Alignment
and browsing remain separate interaction states: passage selection controls the
persistent blue aligned-page marker, while scrolling controls which centered
page is slightly enlarged. Related slides do not affect either state.

Alignment navigation is bidirectional. Selecting a passage activates its slide
position and highlights every passage sharing that position; selecting a slide
activates the same relation, pauses audio, highlights all matching passages,
and scrolls to the first one without choosing or playing an individual passage.
Slides with no inferred passage position report that state without moving the
transcript. Reverse navigation uses `slide_position`, never `related_slides`.

Audio navigation projects in the other direction through the report's passage
playback intervals. Playing or seeking marks the passage at the playhead and
activates its slide position. Natural playback scrolls only when that passage
leaves the viewport, while an explicit seek centers it. The default single-
passage mode stops after a clicked passage; continuous mode removes that stop.
Manual seeking always cancels a previously selected passage boundary.

`evaluate-analysis` combines accepted projection diagnostics with rejected
partition validations from `model-trace.jsonl`. It reports exact accepted,
fuzzy accepted, and rejected candidate-attempt rates, rejected affected
windows, aggregate accepted copy differences, and final-answer repair turns.
Exact/fuzzy outcomes describe final accepted windows; rejected counts describe
earlier attempts that were repaired and therefore do not appear in the final
artifact.

## 6. Model transport and observability

The model adapter targets an explicitly configured OpenAI-compatible Chat
Completions endpoint through `genai`; model names are not used to infer a
provider. JSON mode is requested, while deterministic schema and domain
validation remain local.

The first window of each windowed stage runs alone as a canary. Comparative
ranking instead accepts one batch for importance and one for novelty before
launching either metric's remaining batches with bounded concurrency. Request
attempts pass through a shared `RequestScheduler`. The CLI defaults to adaptive
mode: start at two requests and grow gradually to a hard ceiling of eight when
healthy demand is queued. `BEYOND_SLIDES_SCHEDULING` selects adaptive or fixed;
`BEYOND_SLIDES_MAX_CONCURRENCY` sets the ceiling, and
`BEYOND_SLIDES_REQUEST_INTERVAL_MS` sets a request-start spacing floor.

HTTP 429 reduces the adaptive cap and learned sending rate and extends a shared
cooldown. Generation-tagged feedback prevents one already-dispatched throttled
wave from repeatedly halving the cap; stale successes cannot regrow it. Healthy
epochs recover the constraining gate gradually. Latency helps compare the two
gates, but is not itself treated as evidence of congestion. Fixed mode keeps
its configured cap/spacing while still honoring shared cooldowns.

All retries, repairs, and tool follow-ups acquire the same cancellation-safe
attempt guard. Waiting and retry sleeps hold no HTTP slot, and pacing slots are
committed only at admission. The CLI shares the scheduler across sequential
stages in its configured provider scope; separate processes do not coordinate.
Timeouts, HTTP 408/429, transport failures, and 5xx responses retain bounded
retries with jitter. Retry-After supports seconds and dates without shortening
valid long delays. A 5xx suppresses growth without automatically learning a
quota. This is a bounded policy, not universal rate-limit discovery; see the
research and experiment under `docs/`.

`BEYOND_SLIDES_CHAT_EXTRA_BODY` supplies optional provider-specific JSON.
`BEYOND_SLIDES_RESTORATION_CHAT_EXTRA_BODY` and
`BEYOND_SLIDES_ANNOTATION_CHAT_EXTRA_BODY` may override it per stage. This lets
an editorial restoration pass disable expensive reasoning while semantic
annotation retains it.

Every provider attempt appends typed JSONL events for request, response,
provider error, validation, or processing failure. Records correlate an
exchange with workflow, zero-based work-item index, conversation turn, and
request kind. The trace's legacy `window_index` field is a transcript-window
index for restoration and passage preparation, and a batch-plan index for
comparative workflows. API keys and authorization headers are never recorded.
Complete records are flushed individually. On resume, a malformed
non-newline-terminated crash tail is truncated; corruption in any completed
line is rejected.

## 7. Run layout and resumability

```text
run/lecture-analysis/
|-- manifest.json
|-- execution-settings.json
|-- request-scheduling.json
|-- model-trace.jsonl
|-- window-0001.json
|-- window-0002.json
|-- ...
|-- comparisons/
|   |-- importance-<configuration-hash>/
|   |   |-- manifest.json
|   |   |-- importance-batch-0001.json
|   |   `-- ...
|   `-- novelty-<configuration-hash>/
|       |-- manifest.json
|       |-- novelty-batch-0001.json
|       `-- ...
|-- analysis.json
|-- annotation-quality.json
|-- report.html
|-- timed-tokens.json (optional input)
|-- report.assets/
|   |-- lecture-audio.flac
|   `-- slides/
|       |-- slide-0001.png
|       `-- ...
`-- restoration/
    |-- manifest.json
    |-- execution-settings.json
    |-- request-scheduling.json
    |-- model-trace.jsonl
    |-- window-0001.json
    |-- ...
    |-- restored-transcript.json
    |-- restored-transcript.txt
    `-- diagnostics.json
```

Restoration has its own manifest, trace, and checkpoints. Passage preparation
has a root manifest and window checkpoints; its trace also records comparative
requests. Each comparison metric has a manifest and checkpoint namespace bound
to the prepared passage content, upstream semantic configuration, its own
prompt, and its grouping/evidence configuration. Changing a comparison prompt
creates a new namespace for that metric without rerunning upstream stages or
deleting the old comparisons.

The legacy root manifest remains readable and is not rewritten on resume.
Compatibility is stage-specific: source hashes, relevant prompt/model/provider
settings, windowing, retrieval, and output/tool limits still bind checkpoints.
Concurrency, request pacing, and bounded retry/repair budgets do not invalidate
an already accepted result. `execution-settings.json` records the latest
mode, concurrency ceiling, and pacing floor separately. `request-scheduling.json`
records current-invocation operational telemetry at checkpoints and stage ends;
it is not checkpoint identity or a historical trace. Every reused result still undergoes its
normal domain validation; an unmanifested checkpoint directory is not adopted.
Secrets are excluded. See `docs/running.md` for commands and resume behavior.

When a recording is supplied for rendering, the report stages it under
`report.assets/` (normally as a hard link). Passage selection uses projected
timed-token boundaries when an optional timing sidecar supports a reliable
local match, and otherwise uses the original segment timestamps. Playback
stops at the selected passage's derived or fallback end.

Window indices in core code are zero-based. Filenames and user-facing progress
translate them to one-based window numbers only at the boundary.

## 8. Testing and evaluation policy

Synthetic fixtures test domain invariants, Chinese retrieval, restored
windowing, exact and fuzzy projection, rejection at the 5% boundary, tool
sessions, comparative group coverage and aggregation, provider conversations,
resumability, artifact validation, and HTML escaping. Mock endpoints verify
tool-budget enforcement, structured repairs, rate-limit retry, request pacing,
and trace records.

Real-course evaluation separates uncertain components:

- visual reference versus semantic slide positions;
- labeled slide retrieval recall;
- restoration evidence review;
- exact/fuzzy/rejected passage-partition rates;
- human review of comparative novelty and importance stability, and of
  connection strength;
- whether continuous score typography helps a learner locate useful oral
  additions without destroying lecture context.

Thresholds, retrieval fusion, transition penalties, and score prompts should be
tuned from labeled evidence rather than one aesthetically pleasing report.

## 9. Known limits and next decisions

- A model can preserve wording yet choose poor semantic passage boundaries;
  projection verifies source fidelity, not judgment quality.
- Comparative percentiles improve within-lecture separation by construction;
  they do not reveal whether a lecture is uniformly novel or uniformly useful.
- Slide-level evidence may prove too coarse; exact slide excerpts can be added
  if evaluation requires them.
- Dense retrieval currently loads one fixed local Chinese embedding model.
- The default slide-position prior assumes a complete recording beginning at
  the first slide.
- PDF text warnings do not yet trigger OCR or multimodal ingestion.
- Fine passage playback timing requires an optional SenseVoice timing sidecar;
  reports without it retain coarse, potentially overlapping segment intervals.
- The two model stages share orchestration concepts but remain distinct
  sessions until connecting or extracting them produces a genuinely deeper
  interface rather than a generic workflow framework.
- Traditional Chinese and broader multilingual export remain future work.
