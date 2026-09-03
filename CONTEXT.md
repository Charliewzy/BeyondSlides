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
The ordered, timestamped textual record of a lecture.
_Avoid_: Notes, lecture summary

**Transcript segment**:
The smallest individually timestamped and referenced part of a transcript.
_Avoid_: Transcript sentence, ASR segment, utterance

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
written source.
_Avoid_: Uniqueness

**Connection strength**:
The degree to which a lecture passage makes a useful conceptual connection to
content elsewhere in the written source.
_Avoid_: Relatedness

**Importance**:
The learning value of a lecture passage for understanding or applying the
course.
_Avoid_: Novelty, relevance
