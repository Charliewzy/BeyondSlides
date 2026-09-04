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

Novelty, connection strength, and importance are graded from 0 through 5. They
remain uncertain semantic judgments, not objective facts. Optional comparison
notes support debugging and evaluation but are neither required nor shown to a
learner by default.

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

Restoration and annotation each persist validated window checkpoints before
counting them complete. Run manifests bind checkpoints to source identities,
prompts, models, and deterministic configuration. Complete model exchanges are
recorded without credentials.

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
    related_slides: Vec<SlideId>,
    summary: Option<String>,
    comparison_note: Option<String>,
}
```

Lecture passages partition the readable restored transcript in chronological
order. Their `text` is authoritative restored text, never trusted model copy.
All scores are validated inclusive `0..5` values. Related slide IDs must exist
and contain no duplicates. Summary and comparison note remain optional.

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
            model annotation + slide tools
                         |
                         v
             fuzzy boundary projection
                         |
                         v
             validation and assembly
                         |
                         v
              artifact and HTML report
```

### 5.1 Ingestion and normalization

The FunASR TSV adapter converts each nonblank timestamped row into one
zero-based transcript segment. It preserves ASR evidence instead of inventing
grammatical boundaries. The PDF adapter uses Poppler `pdftotext` in raw reading
order, preserves one slide per page, removes only rigorously detected repeated
page furniture, and reports sparse text or suspicious glyphs rather than
silently invoking OCR.

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

### 5.5 Annotation conversation

The model receives left context, owned readable text, right context, inferred
slide position, and a nearby slide neighborhood. It may call:

```text
inspect_slide(slide_id)
search_slides(query, max_results)
```

Search covers the entire deck so the model can challenge an apparent novelty
claim. A per-window tool session suppresses duplicate slide text after first
exposure. Tool rounds are bounded. After the last permitted tool result, the
client removes tool definitions and tool choice from the next request, forcing
a final-answer attempt at the protocol level rather than trusting the model to
count rounds.

The model returns proposed passages containing copied text and judgments. It
never supplies trusted offsets or raw source IDs.

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

### 5.7 Assembly and persisted artifacts

Per-window checkpoints contain source-backed passages, model diagnostics, and
projection diagnostics. Restoring a checkpoint reruns deterministic
window-local validation before trusting it. Assembly orders window results,
validates each partition again, then validates the complete restored transcript
and lecture-wide passage partition through the same shared partition routine.

`analysis.json` is a `RestoredAnalysisArtifact` containing:

- the complete restored transcript;
- all chronological lecture passages, each carrying its inferred slide
  position;
- per-window model diagnostics;
- per-window projection diagnostics.

The offline `render-analysis` command revalidates that artifact against the raw
transcript and slide deck before rendering. Metadata arrays must agree on their
window count, slide positions must exist, and passage text/provenance must still
match the restored transcript.

### 5.8 Rendering and evaluation

The continuous report renders authoritative passage text in lecture order.
Importance maps to six font weights and novelty maps to six underline
thicknesses. Hover, focus, or click reveals timestamps, raw source range,
inferred slide position, and component scores.

An optional PDF presentation adapter invokes Poppler once at rendering time and
writes one ordered PNG per normalized slide beneath a report-local asset
directory. The slide count must exactly match the normalized slide deck. The
HTML references only those local images and embeds no remote assets. Alignment
and browsing remain separate interaction states: passage selection controls the
persistent blue aligned-page marker, while scrolling controls which centered
page is slightly enlarged. Related slides do not affect either state.

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

The first window of each stage runs alone as a canary. After it succeeds,
remaining windows run with bounded concurrency. Annotation request starts are
paced across concurrent conversations. Timeouts, HTTP 408/429, transport
failures, and 5xx responses have a bounded retry policy; numeric `Retry-After`
is respected, while 429 without that header uses longer exponential backoff.

`BEYOND_SLIDES_CHAT_EXTRA_BODY` supplies optional provider-specific JSON.
`BEYOND_SLIDES_RESTORATION_CHAT_EXTRA_BODY` and
`BEYOND_SLIDES_ANNOTATION_CHAT_EXTRA_BODY` may override it per stage. This lets
an editorial restoration pass disable expensive reasoning while semantic
annotation retains it.

Every provider attempt appends typed JSONL events for request, response,
provider error, validation, or processing failure. Records correlate an
exchange with workflow, zero-based window index, conversation turn, and request
kind. API keys and authorization headers are never recorded. Complete records
are flushed individually. On resume, a malformed non-newline-terminated crash
tail is truncated; corruption in any completed line is rejected.

## 7. Run layout and resumability

```text
run/lecture-analysis/
|-- manifest.json
|-- model-trace.jsonl
|-- window-0001.json
|-- window-0002.json
|-- ...
|-- analysis.json
|-- annotation-quality.json
|-- report.html
|-- report.assets/
|   `-- slides/
|       |-- slide-0001.png
|       `-- ...
`-- restoration/
    |-- manifest.json
    |-- model-trace.jsonl
    |-- window-0001.json
    |-- ...
    |-- restored-transcript.json
    |-- restored-transcript.txt
    `-- diagnostics.json
```

Each stage has its own manifest, trace, and checkpoints. Manifests record source
hashes, embedded prompt hash, endpoint and model identity, optional extra body,
windowing, concurrency, retrieval, request pacing, retry limits, and output
limits. Secrets are excluded. A directory resumes only when the requested
configuration exactly matches its manifest.

Window indices in core code are zero-based. Filenames and user-facing progress
translate them to one-based window numbers only at the boundary.

## 8. Testing and evaluation policy

Synthetic fixtures test domain invariants, Chinese retrieval, restored
windowing, exact and fuzzy projection, rejection at the 5% boundary, tool
sessions, provider conversations, resumability, artifact validation, and HTML
escaping. Mock endpoints verify tool-budget enforcement, structured repairs,
rate-limit retry, request pacing, and trace records.

Real-course evaluation separates uncertain components:

- visual reference versus semantic slide positions;
- labeled slide retrieval recall;
- restoration evidence review;
- exact/fuzzy/rejected passage-partition rates;
- human review of novelty, connection strength, and importance;
- whether continuous score typography helps a learner locate useful oral
  additions without destroying lecture context.

Thresholds, retrieval fusion, transition penalties, and score prompts should be
tuned from labeled evidence rather than one aesthetically pleasing report.

## 9. Known limits and next decisions

- A model can preserve wording yet choose poor semantic passage boundaries;
  projection verifies source fidelity, not judgment quality.
- Slide-level evidence may prove too coarse; exact slide excerpts can be added
  if evaluation requires them.
- Dense retrieval currently loads one fixed local Chinese embedding model.
- The default slide-position prior assumes a complete recording beginning at
  the first slide.
- PDF text warnings do not yet trigger OCR or multimodal ingestion.
- The two model stages share orchestration concepts but remain distinct
  sessions until connecting or extracting them produces a genuinely deeper
  interface rather than a generic workflow framework.
- Traditional Chinese and broader multilingual export remain future work.
