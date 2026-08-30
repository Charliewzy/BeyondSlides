# BeyondSlides Architecture

Status: Draft for review

## 1. Product goal

BeyondSlides finds the useful information a lecturer contributes orally beyond
the course's written sources.

It is not a general lecture summarizer. Given a timestamped lecture transcript
and the associated slide deck, it should identify and rank explanations,
intuitions, caveats, examples, practical advice, and conceptual connections
that are valuable to a learner but are not already explicit in the slides.

Every retained result must be internally traceable from both directions:

- what the lecturer said: an exact transcript span and timestamps;
- what written-source content was considered: the related slides.

An optional comparison note may be retained for logging, debugging, and
evaluation. It is not required for a valid analysis or in the user-facing
report.

The central operation is therefore a semantic difference:

```text
useful lecture content - what the slides already communicate
    = oral additions worth the learner's time
```

## 2. MVP user experience

The first useful version accepts two normalized files:

- `transcript.json`: ordered, timestamped sentences;
- `slides.json`: ordered slide texts.

It produces:

- `annotations.json`: validated, evidence-bearing judgments over the complete
  transcript;
- `result.html`: a self-contained report with two views.

The report's primary view is a ranked list of high-value oral additions. Each
item shows its score, timestamp, transcript evidence, and related slides. An
optional summary may provide a shorter user-facing description when available.

The secondary view preserves the complete transcript and overlays its scores.
This view makes the analysis inspectable and lets a learner browse lower-ranked
material without losing the lecture's original context.

### Initial inclusion rule

The ranked view initially includes a group when:

```text
importance >= 3
and (novelty >= 2 or connection_strength >= 3)
```

This is a configurable presentation rule, not part of the meaning of the
scores. Evaluation on real lectures may change the default.

## 3. MVP scope

The first vertical slice starts from hand-authored JSON fixtures. It includes:

- typed Rust domain models;
- strict input and annotation validation;
- ranking and filtering;
- self-contained HTML rendering;
- a small synthetic lecture used as an integration fixture.

The following are deliberately deferred:

- audio/video transcription and speaker diarization;
- PDF, PPTX, OCR, or textbook ingestion;
- monotonic slide alignment;
- LLM or agent API calls;
- a server, database, or frontend framework;
- user accounts, collaboration, and deployment.

The initial language target is Simplified Chinese, including the Latin
technical terms, formulas, and identifiers commonly mixed into Chinese course
material. General multilingual support and foreign-student export are outside
the initial scope.

Deferring these features lets the first slice validate the output contract and
product experience before adding expensive or uncertain machinery.

## 4. Architectural principles

### Preserve sources

Transcript and slide content are immutable source records. Later stages refer
to canonical typed IDs rather than copying or rewriting source text.

### Evidence-backed analysis

Every novelty judgment internally records its transcript span and relevant
slide references. It may also include a concise comparison note for logging,
debugging, and evaluation. The note is a short justification of the structured
judgment, not a request for the model's private chain of thought.

### Grade, do not force a binary answer

Material can be explicit, paraphrased, implied, meaningfully elaborated, or
genuinely absent from the slides. A `0..5` novelty score represents this
continuum.

### Separate chronological position from semantic relevance

Monotonic alignment will answer, "Where are we in the slide sequence?" Global
retrieval will answer, "Where else do the slides discuss this idea?" An aligned
slide neighborhood is a prior for the agent, not proof of relevance.

### Make expensive stages resumable

Each major stage writes a readable artifact. Rendering or prompt changes must
not require transcription, embedding, or earlier LLM work to run again.

### Prefer deterministic structure around probabilistic judgments

Window construction, alignment, schema validation, coverage checks, ranking,
and rendering are deterministic Rust stages. The model is reserved for semantic
comparison and annotation.

### Build one vertical slice at a time

BeyondSlides begins as one Rust crate with small modules. It should not become a
workspace, distributed service, or plugin system without a demonstrated need.

## 5. Domain model

The examples below describe the intended model, not a frozen Rust API.

### Canonical source identifiers

```rust
struct SentenceId(u32);
struct SlideId(u32);
```

Sentence and slide IDs are typed, zero-based collection positions. For every
normalized source, `sentences[i].id == SentenceId(i)` and
`slides[i].id == SlideId(i)`. They remain stable within that normalized source
snapshot; changing normalization invalidates downstream artifacts. If later
ingestion requires identity across normalization runs, it must add a separate
source reference rather than weakening this invariant. Human-facing sentence,
slide, and PDF page numbers may remain one-based and are not IDs. Timestamps are
integer milliseconds.

### Transcript

```rust
struct Sentence {
    id: SentenceId,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

struct Transcript {
    sentences: Vec<Sentence>,
}
```

Required invariants:

- each ID equals the sentence's zero-based collection position;
- text is non-empty;
- `start_ms <= end_ms`;
- sentence times are nondecreasing;
- transcript order agrees with timestamp order.

### Slides

```rust
struct Slide {
    id: SlideId,
    text: String,
}

struct SlideDeck {
    slides: Vec<Slide>,
}
```

Required invariants:

- each ID equals the slide's zero-based collection position;
- slide order is stable;
- empty extracted text is allowed because image-only slides may exist later.

### Scores

All three scores use a validated inclusive `0..5` type.

`novelty` asks how much useful content in the span is absent from the entire
slide deck:

| Score | Meaning |
| --- | --- |
| 0 | Directly stated in the slides |
| 1 | Essentially a paraphrase |
| 2 | A meaningful elaboration, but mostly implied |
| 3 | Substantial additional explanation or detail |
| 4 | Largely absent from the slides |
| 5 | Genuinely new and non-obvious relative to the slides |

`connection_strength` asks how strongly the span makes a useful conceptual
connection to material elsewhere in the slide deck:

| Score | Meaning |
| --- | --- |
| 0 | No meaningful connection |
| 1 | Weak or incidental relationship |
| 2 | Relevant relationship with limited learning value |
| 3 | Clear, useful connection |
| 4 | Strong connection that improves understanding |
| 5 | Major synthesis across concepts or topics |

This score intentionally has no connection direction or type.

`importance` asks how valuable the spoken material is for understanding or
applying the course:

| Score | Meaning |
| --- | --- |
| 0 | Filler or irrelevant material |
| 1 | Minor detail or low-value anecdote |
| 2 | Helpful but nonessential detail |
| 3 | Clearly useful explanation or advice |
| 4 | Important conceptual or practical insight |
| 5 | Central insight with high leverage for learning or application |

### Lecture passages

A lecture passage covers a contiguous, inclusive sentence range:

```rust
struct LecturePassage {
    start: SentenceId,
    end: SentenceId,
    novelty: Score5,
    connection_strength: Score5,
    importance: Score5,
    related_slides: Vec<SlideId>,
    summary: Option<String>,
    comparison_note: Option<String>,
}
```

`summary` is an optional user-facing description of a lecture passage. Its
absence does not make a passage invalid; a renderer can use the source
transcript instead. `comparison_note` briefly compares the span with the closest
written source when such a note is useful. It is optional, stored in
`annotations.json` for internal logging, debugging, and evaluation, and not
rendered in the user-facing report.

The source evidence for a passage is the referenced sentence range itself. Its
written-source evidence is `related_slides`; an optional `comparison_note` may
supplement it. A later schema may add exact slide excerpts if evaluation shows
that slide-level references are not sufficiently auditable.

Required invariants:

- start and end IDs exist and follow transcript order;
- all scores are valid;
- all related slide IDs exist;
- related slide IDs contain no duplicates.

## 6. Window ownership

Long transcripts are analyzed in windows. Each window has overlapping context
but owns a non-overlapping transcript region:

```text
              visible to the model
     +------------------------------------+
     | left context | owned | right context |
     +------------------------------------+
                      ^^^^^
              only this region is annotated
```

Owned regions partition the transcript. The model must partition its entire
owned region into contiguous lecture passages. Context sentences may influence
the judgment but may not appear in that window's output.

For every owned region, validation enforces:

- the first passage starts at the first owned sentence;
- the last passage ends at the last owned sentence;
- consecutive passages are adjacent;
- no sentence is skipped or covered twice.

Consequently, after all windows finish, every transcript sentence belongs to
exactly one lecture passage. No overlap-merging stage is needed.

## 7. Intended pipeline

The complete system is organized as the following stages:

```text
recording             slide deck
    |                      |
    v                      v
transcription         text extraction
    |                      |
    +------> normalized sources <------+
                       |
                       v
              transcript windowing
                       |
          +------------+-------------+
          |                          |
          v                          v
  all-slide scoring         semantic alignment
          |                          |
          +------------+-------------+
                       v
              agent annotation
          local slide prior + global search
                       |
                       v
              validate and assemble
                       |
                       v
                rank and render
```

### 7.1 Source normalization

Adapters convert external formats into `Transcript` and `SlideDeck`. The core
pipeline depends only on these normalized source data structures. The initial
fixture bypasses all adapters. `ValidatedSources` accepts the normalized
transcript and slide deck after enforcing their invariants, allowing windowing
and other pre-annotation stages to operate on trusted sources.

The FunASR TSV adapter preserves evidence rather than inventing grammatical
boundaries: every nonblank `start`, `end`, `text` row becomes one transcript
sentence with a sequential zero-based ID. It converts decimal seconds exactly to
milliseconds, trims surrounding text whitespace, and rejects malformed,
reversed, or overlapping rows. Punctuation restoration or sentence merging, if
added later, must remain a separate transformation with traceable source
evidence.

The first PDF adapter uses Poppler's `pdftotext` in raw reading order. Every PDF
page becomes one slide with a canonical zero-based ID, including pages whose
extracted text is empty. Human-facing page numbers in warnings remain one-based.
The adapter removes only a trailing line whose page-counter-normalized form
occurs on a strict majority and at least three pages. It reports sparse text and
aggregates suspicious glyphs per page; these warnings do not trigger OCR or
silently drop source evidence. Plain-text sufficiency remains an evaluation
decision outside the adapter.

### 7.2 Windowing

The transcript is divided into owned regions with left and right context. The
current policy greedily adds complete transcript sentences while both a text
character budget and a wall-clock duration budget permit it. A single sentence
that exceeds either budget remains intact in its own owned region. Left and
right context also contain only complete sentences and each has its own
character budget. A later agent adapter may translate its model-specific token
budget into these model-independent source-window limits; the correctness
contract remains based on sentence ownership rather than token counts.

### 7.3 Retrieval

Slides are indexed once behind one interface:

```rust
trait SlideScorer {
    fn score_slides(&self, query: &str) -> Result<Vec<SlideScore>, SearchError>;
}
```

Every adapter returns exactly one score per slide in presentation order,
including zero scores where it found no evidence. The result is fallible because
local model loading and inference can fail. Such a failure must not be
misrepresented as "no related slide found." Dynamic programming consumes the
complete score rows. Callers that need search results sort a copy and truncate
it; consequently, `max_results` belongs to the agent's search tool rather than
the scoring interface.

Three in-memory adapters currently implement this interface:

- lexical retrieval uses BM25, Unicode compatibility normalization, and
  Jieba's Chinese search-mode segmentation. It preserves Latin technical terms,
  numbers, and identifiers and omits slides with no matching term;
- dense retrieval uses `BAAI/bge-small-zh-v1.5` through FastEmbed. It embeds
  every non-empty slide once, adds the model's recommended Chinese retrieval
  instruction only to queries, and brute-forces cosine similarity;
- hybrid retrieval combines the complete lexical and dense rankings with
  equal-weight reciprocal-rank fusion using `k = 60`. Rank fusion avoids
  treating BM25 and cosine scores as though they shared a scale.

All adapters return slides in presentation order; ranking is a caller-side
operation. Model files are downloaded into FastEmbed's local cache on first use
and reused afterward. The model identifier and scoring mode must eventually be
recorded in the run manifest.

### 7.4 Semantic alignment

Every transcript window is scored against every slide. Each row is normalized
independently to `0..1`, preventing BM25, cosine, or reciprocal-rank-fusion
scales from implicitly changing the transition policy. Dynamic programming then
selects one slide position per window by maximizing semantic evidence minus
transition costs.

The path begins at the first slide for a complete lecture recording. Staying
near the previous position is preferred: movement within three slides receives
a small distance cost. Backward movement is allowed at the same cost as forward
movement, and larger jumps remain possible with a larger soft penalty. Support
for recordings that begin mid-lecture will require making the starting prior
configurable.

The resulting slide position supplies chronological context. It is not itself
a claim that the selected slide is a related slide.

A self-contained alignment visualization makes the complete score matrix
inspectable. It renders transcript windows horizontally, slides vertically,
normalized scores as a heatmap, and the inferred path as an overlay. Selecting
a window reveals its transcript, highest semantic scores, and the inferred
slide neighborhood. When optional visual reference evidence exists, the report
adds a separate dotted path and mismatch markers; the production alignment does
not consume that reference.

### 7.5 Optional visual reference alignment

BeyondSlides does not require the original lecture video. When it is available,
visual matching can supply reference evidence for evaluating semantic slide
positions. Poppler renders every PDF page at `320x180`; FFmpeg
streams one grayscale video frame per second at the same dimensions; and Rust
compares each frame with every page using MSSIM. The matcher records the best
and runner-up slide and their scores for every sampled timestamp. Frames are
processed as a stream rather than materialized as image files, and callers may
receive processed and total video time for progress reporting.

This first implementation deliberately preserves raw observations. It does not
yet convert a small score margin into false confidence, smooth transient
mismatches, or refine transition timestamps. A later temporal decoder may add
those policies, but it must permit backward movement because lecturers can
revisit earlier slides.

Visual reference evidence is not fed into the production semantic alignment
path. It measures whether inferred slide positions match what was displayed,
while manually labeled related slides remain necessary to evaluate semantic
retrieval.

### 7.6 Agent annotation

For each window, the agent initially receives:

- left, owned, and right transcript regions;
- the locally aligned slide neighborhood;
- the score definitions and output schema.

It may use a deliberately small tool set:

```text
inspect_slide(slide_id)
search_slides(query, max_results)
```

Search covers the entire deck so the agent can try to falsify an apparent
novelty judgment. Tool rounds and calls are bounded. Independent windows may be
processed concurrently after alignment.

### 7.7 Validation and assembly

Model output is untrusted input. Rust code validates its schema, score ranges,
IDs, owned-region partition, and evidence requirements before accepting it.
Invalid output may be retried with the validation errors; persistent failure is
recorded explicitly rather than silently patched into a plausible result.
Each model response has the following shape:

```rust
struct TranscriptWindowAnalysis {
    passages: Vec<LecturePassage>,
}
```

Responses are supplied in transcript-window order. Assembly rebuilds the
deterministic window assignments from the recorded `WindowingConfig`, requires
exactly one response per window, and requires the response's lecture passages
to partition exactly that window's owned region. A response cannot claim
sentences from its left or right context. Only after these window-local checks
pass are all passages joined and subjected to lecture-wide source, related-slide,
and coverage validation.
`ValidatedAnalysis` combines already-validated sources with the accepted
lecture passages, so source validation is not repeated after annotation.

### 7.8 Ranking and rendering

The initial ranked view sorts primarily by importance, then novelty, then
connection strength. It should retain the component scores rather than collapse
them into a pseudo-precise aggregate number.

Rendering is deterministic and consumes only normalized sources and validated
annotations. The initial renderer uses plain HTML, CSS, and JavaScript and emits
one portable file.

## 8. Run artifacts and resumability

Each real analysis run will use a directory such as:

```text
run/
|-- manifest.json
|-- transcript.json
|-- slides.json
|-- windows.json
|-- retrieval-index/
|-- alignment.json
|-- annotations.json
`-- result.html
```

`manifest.json` records input identities, schema versions, configuration, stage
status, and the model/provider identifiers needed to understand how the result
was produced. Secrets must never be written to the run directory.

An artifact is reused only when its schema version and relevant upstream inputs
match. The exact cache-key scheme is deferred until a second expensive stage is
implemented.

## 9. Error and uncertainty policy

- Malformed normalized inputs fail before analysis begins.
- Missing slide text is represented, not invented.
- Invalid model output is rejected with actionable validation errors.
- A failed window remains visibly failed; it is not treated as low novelty.
- When present, comparison notes use calibrated language such as "not found in
  the inspected slides" when the evidence does not justify an absolute absence
  claim.
- The UI always lets the user inspect the underlying transcript and referenced
  slides.

## 10. Testing and evaluation

### Synthetic fixture

The permanent `tiny_course` fixture should contain at least:

- natural Simplified Chinese without artificial spaces between words;
- mixed Latin technical terms or formulas;
- direct repetition of a slide;
- a valuable oral explanation absent from the slide wording;
- a useful connection to an earlier slide;
- a novel but unimportant anecdote;
- adjacent sentences that must be grouped together.

The first automated tests verify deserialization, domain invariants, complete
annotation coverage, ranking behavior, and successful HTML generation.

### Later component evaluation

Once the deterministic slice works, evaluate each uncertain component
separately:

- alignment accuracy against manually paired windows and slides;
- retrieval recall for known relevant slides;
- novelty, connection, and importance agreement with human labels;
- end-to-end usefulness: how much lecture time can a learner skip without
  missing instructor-added value?

Evaluation should use a manually annotated portion of a real lecture before
prompts, thresholds, or alignment penalties are heavily tuned.

The synthetic `retrieval_course` fixture provides a reproducible development
comparison across semantic-paraphrase, exact-term, and mixed Chinese queries.
Its metrics guard the evaluation procedure, not a claim that one retrieval mode
is generally superior. Model or fusion decisions require the real-lecture
evaluation above.

## 11. Implementation sequence

1. Add normalized source data structures and the `tiny_course` JSON fixture.
2. Validate transcript, slides, scores, evidence, and exact passage coverage.
3. Rank qualifying lecture passages and render the two-view self-contained HTML
   report.
4. Add deterministic owned-region windowing.
5. Add lexical slide search, followed by dense retrieval and hybrid fusion.
6. Add soft non-monotonic window-to-slide alignment and a human-readable
   visualization.
7. Add schema-constrained model annotation using only local aligned slides.
8. Add global `inspect_slide` and `search_slides` agent tools.
9. Add real PDF slide extraction.
10. Add an external transcription adapter and sentence normalization.
11. Evaluate on a manually labeled lecture segment and tune from evidence.

The first acceptance checkpoint is intentionally smaller than the complete
pipeline:

> Given the synthetic `transcript.json`, `slides.json`, and `annotations.json`,
> BeyondSlides validates an exact partition of the transcript, ranks useful oral
> additions, and generates a self-contained report whose claims can be traced
> back to transcript and slide evidence.

## 12. Deferred decisions

The following choices should be made when the corresponding milestone begins:

- exact JSON schema versioning and migration policy;
- word-, token-, or duration-based window sizing;
- whether a larger or newer embedding model justifies its additional cost;
- retrieval fusion tuning and alignment scoring parameters based on real labels;
- Traditional Chinese normalization and general multilingual analysis;
- LLM provider and structured-output protocol;
- PDF/PPTX extraction backends;
- whether textbooks become a second written source in the MVP's successor;
- whether slide-level evidence is sufficient or exact slide excerpts are
  required;
- visualization details and accessibility palette.

These choices are intentionally absent from the core architecture because none
changes the initial domain contract or first vertical slice.
