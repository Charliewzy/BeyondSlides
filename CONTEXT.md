# BeyondSlides

BeyondSlides identifies the valuable contribution a lecturer makes beyond the
course's written sources. This context defines the language used to compare a
spoken lecture with those sources.

## Source material

**Written source**:
Course material that establishes what a learner can obtain without listening to
the lecture. A slide deck is the first supported written source; a textbook may
become another.
_Avoid_: Written material, reference material

**Transcript**:
The ordered textual record of a lecture, with recording timestamps when the
source provides them.
_Avoid_: Notes, lecture summary

**Transcript segment**:
The smallest individually referenced part of a transcript. Its recording
interval is optional; an untimed text source does not establish audio timing.
_Avoid_: Transcript sentence, ASR segment, utterance

**Timed transcript token**:
An optional fine-grained ASR text unit with its own recording interval, used to
derive playback boundaries. A token is commonly one Chinese character but may
contain several Latin characters; it is not assumed to be a linguistic word.
_Avoid_: Word timestamp, exact transcript position

**Restored transcript**:
Readable lecture text derived from a transcript while retaining references to
the transcript segments that support it.
_Avoid_: Clean transcript, corrected transcript

**Restored transcript span**:
A contiguous range of transcript segments represented as readable text or
explicitly omitted as disfluency. Its text may contain part of one grammatical
sentence or several grammatical sentences.
_Avoid_: Restored sentence, paraphrase

**Lecture passage**:
A contiguous part of the readable restored transcript evaluated as one
meaningful unit. It retains the transcript-segment range supporting its text,
even when that source range is coarser than the passage boundary.
_Avoid_: Annotation group, segment group

**Passage playback interval**:
The approximate recording interval played for a lecture passage. It is derived
from timed transcript tokens when reliable text alignment is available and
otherwise falls back to the passage's transcript-segment range. It is not
source provenance.
_Avoid_: Passage source range, exact timestamp

## Windowing

**Transcript window**:
The part of a transcript visible during one restoration task, consisting of one
owned region and its optional left and right context.
_Avoid_: Chunk, annotatable window

**Restored transcript window**:
The part of a restored transcript visible during one analysis task. Its regions
contain complete restored transcript spans so their provenance remains intact.
_Avoid_: Restored chunk, cleaned window

**Owned region**:
A contiguous range for which exactly one window is responsible. The owned
regions of a window sequence together partition its transcript representation.
_Avoid_: Owned window, annotation span

## Analysis

**Model backend**:
The authenticated execution adapter that turns provider-neutral lecture tasks
into validated structured model results. Current backends are an explicitly
configured OpenAI-compatible endpoint and a local Codex app-server using the
user's cached ChatGPT sign-in.
_Avoid_: Provider (when referring to our adapter), model endpoint (for Codex)

**Cut cost**:
An ordinal `0..5` judgment of how much semantic or grammatical damage would be
caused by dividing restored transcript text at one candidate gap. Zero is a
natural division and five is severely damaging; no value makes a gap
absolutely forbidden.
_Avoid_: Boundary strength, confidence, break score

**Oral addition**:
A lecture passage whose explanation, insight, caveat, example, advice, or
connection provides meaningful learning value beyond the written source.
_Avoid_: BTW, delta, annotation

**Slide position**:
An approximate chronological location in the slide deck inferred for a lecture
passage. It provides local context but neither observes which slide was visible
nor establishes semantic relevance.
_Avoid_: Current slide, Corresponding slide

**Related slide**:
A slide whose content is semantically relevant to a lecture passage, regardless
of when that slide appears in the presentation order.
_Avoid_: Current slide

**Comparison note**:
An optional concise internal record of why a lecture passage received its
judgments when compared with the closest written-source content.
_Avoid_: Reason, explanation

**Novelty**:
The degree to which the useful content of a lecture passage is absent from the
written source. Final novelty is ranked relative to other passages in the same
lecture from slide-grounded comparisons; it is not calibrated across lectures.
_Avoid_: Uniqueness

**Connection strength**:
The degree to which a lecture passage makes a useful conceptual connection to
content elsewhere in the written source.
_Avoid_: Relatedness

**Importance**:
The learning value of a lecture passage for understanding or applying the
course. Final importance is ranked relative to other passages in the same
lecture; it is not an absolute or cross-course grade.
_Avoid_: Novelty, relevance

**Comparative judgment**:
A best--worst choice within a small group of lecture passages for exactly one
metric. It names the most and least passage in that group and does not assign
absolute scores.
_Avoid_: Absolute rating, quartet score

**Comparative score**:
The lecture-wide evidence aggregated from a passage's comparative judgments.
It stores comparison count and most and least selections, from which the
best--worst balance and percentile rank are derived.
_Avoid_: Model score, confidence

**Display level**:
A discrete `1..5` band derived from a comparative percentile for report
typography and threshold controls. It is presentation data, not an absolute
semantic measurement.
_Avoid_: Absolute score, rating
