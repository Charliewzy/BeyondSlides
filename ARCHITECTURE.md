# BeyondSlides Architecture

Status: Draft for review

## 1. Product goal

BeyondSlides finds the useful information a lecturer contributes orally beyond
the course's written material.

It is not a general lecture summarizer. Given a timestamped lecture transcript
and the corresponding slides, it should identify and rank explanations,
intuitions, caveats, examples, practical advice, and conceptual connections
that are valuable to a learner but are not already explicit in the slides.

Every retained result must be internally auditable from both directions:

- what the lecturer said: an exact transcript span and timestamps;
- why it counts as added value: the closest relevant slide material and a
  short comparison.

The comparison is retained as an analysis artifact for logging, debugging, and
evaluation. It is not required in the user-facing report.

The central operation is therefore a semantic difference:

```text
useful lecture content - what the slides already communicate
    = oral added value worth the learner's time
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
item shows a concise description, its score, timestamp, transcript evidence,
and related slides.

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
- BM25 and dense retrieval;
- monotonic slide alignment;
- LLM or agent API calls;
- a server, database, or frontend framework;
- user accounts, collaboration, and deployment.

Deferring these features lets the first slice validate the output contract and
product experience before adding expensive or uncertain machinery.

## 4. Architectural principles

### Preserve sources

Transcript and slide content are immutable source records. Later stages refer
to stable IDs rather than copying or rewriting source text.

### Evidence-backed analysis

Every novelty judgment internally records its transcript span, relevant slide
evidence, and a concise comparison note. This information supports logging,
debugging, and evaluation; it is not required in the user-facing report. The
note is a short justification of the structured judgment, not a request for the
model's private chain of thought.

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

### Stable identifiers

```rust
struct SentenceId(u32);
struct SlideId(u32);
struct WindowId(u32);
```

IDs are stable within one run. Timestamps are integer milliseconds. Transcript
and slide collections are stored in presentation order, but code should not use
collection indices as implicit IDs.

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

- IDs are unique;
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

- IDs are unique;
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

### Annotation groups

An annotation covers a contiguous, inclusive sentence range:

```rust
struct AnnotationGroup {
    start: SentenceId,
    end: SentenceId,
    novelty: Score5,
    connection_strength: Score5,
    importance: Score5,
    related_slides: Vec<SlideId>,
    summary: Option<String>,
    comparison_note: String,
}
```

`summary` describes the oral contribution in the ranked view. It is required
for groups admitted to that view and optional for low-value groups retained
only to cover the transcript. `comparison_note` briefly compares the span with
the closest written material. It is stored in `annotations.json` for internal
logging, debugging, and evaluation and is not rendered in the user-facing
report.

The source evidence for an annotation is the referenced sentence range itself.
Its written-material evidence is `related_slides` plus `comparison_note`. A
later schema may add exact slide excerpts if evaluation shows that slide-level
references are not sufficiently auditable.

Required invariants:

- start and end IDs exist and follow transcript order;
- all scores are valid;
- all related slide IDs exist;
- related slide IDs contain no duplicates;
- every group has a non-empty comparison note;
- ranked groups have a non-empty summary.

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
owned region into contiguous annotation groups. Context sentences may influence
the judgment but may not appear in that window's output.

For every owned region, validation enforces:

- the first group starts at the first owned sentence;
- the last group ends at the last owned sentence;
- consecutive groups are adjacent;
- no sentence is skipped or covered twice.

Consequently, after all windows finish, every transcript sentence belongs to
exactly one annotation group. No overlap-merging stage is needed.

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
 slide retrieval index      monotonic alignment
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
pipeline depends only on these normalized models. The initial fixture bypasses
all adapters.

### 7.2 Windowing

The transcript is divided into owned regions with left and right context. The
policy should eventually be token-aware, but its correctness contract is based
on sentence ownership rather than token counts.

### 7.3 Retrieval

Slides are indexed once. The intended search implementation combines lexical
and dense retrieval behind one interface:

```rust
trait SlideSearcher {
    fn search(&self, query: &str, max_results: usize) -> Vec<SearchHit>;
}
```

The agent asks for slide search; it does not select or tune the underlying
retrieval algorithms. With a typical course-sized deck, embeddings can remain
in memory and cosine similarity can be brute-forced.

### 7.4 Monotonic alignment

Each transcript window receives an approximate current slide position using a
similarity matrix and dynamic programming. The path cannot move backward and is
penalized for implausibly large forward jumps.

Alignment produces a current position and nearby slides, not a claim that those
slides contain every idea in the window.

### 7.5 Agent annotation

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

### 7.6 Validation and assembly

Model output is untrusted input. Rust code validates its schema, score ranges,
IDs, owned-region partition, and evidence requirements before accepting it.
Invalid output may be retried with the validation errors; persistent failure is
recorded explicitly rather than silently patched into a plausible result.

### 7.7 Ranking and rendering

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
- Explanations use calibrated language such as "not found in the inspected
  slides" when the evidence does not justify an absolute absence claim.
- The UI always lets the user inspect the underlying transcript and referenced
  slides.

## 10. Testing and evaluation

### Synthetic fixture

The permanent `tiny_course` fixture should contain at least:

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

## 11. Implementation sequence

1. Add normalized models and the `tiny_course` JSON fixture.
2. Validate transcript, slides, scores, evidence, and exact annotation coverage.
3. Rank qualifying groups and render the two-view self-contained HTML report.
4. Add deterministic owned-region windowing.
5. Add lexical slide search, followed by dense retrieval and hybrid fusion.
6. Add monotonic window-to-slide alignment and a human-readable debug view.
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
- concrete BM25 and embedding libraries;
- retrieval fusion and alignment scoring parameters;
- LLM provider and structured-output protocol;
- PDF/PPTX extraction backends;
- whether textbooks become a second written source in the MVP's successor;
- whether slide-level evidence is sufficient or exact slide excerpts are
  required;
- visualization details and accessibility palette.

These choices are intentionally absent from the core architecture because none
changes the initial domain contract or first vertical slice.
