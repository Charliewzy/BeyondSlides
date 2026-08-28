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

**Transcript sentence**:
The smallest individually timestamped and referenced part of a transcript.
_Avoid_: ASR segment, utterance

**Lecture passage**:
A contiguous part of the transcript evaluated as one meaningful unit. Every
transcript sentence belongs to exactly one lecture passage after analysis.
_Avoid_: Annotation group, sentence group

## Windowing

**Transcript window**:
The part of a transcript visible during one analysis task, consisting of one
owned region and its optional left and right context.
_Avoid_: Chunk, annotatable window

**Owned region**:
A contiguous range of transcript sentences for which exactly one transcript
window is responsible. All owned regions together partition the transcript.
_Avoid_: Owned window, annotation span

## Analysis

**Oral addition**:
A lecture passage whose explanation, insight, caveat, example, advice, or
connection provides meaningful learning value beyond the written source.
_Avoid_: BTW, delta, annotation

**Slide position**:
The approximate chronological point in the slide deck being presented during a
lecture passage. It provides local context without claiming semantic relevance.
_Avoid_: Corresponding slide

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
