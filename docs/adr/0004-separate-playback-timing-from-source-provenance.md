# Separate passage playback timing from source provenance

Lecture-passage `source_start` and `source_end` remain the inclusive range of
raw transcript segments supporting the passage. Restored spans can join a
segment's text and annotation can divide that restored text more finely, so
adjacent passages may legitimately share coarse source segments. These fields
therefore cannot also promise exact, non-overlapping audio boundaries.

When available, BeyondSlides ingests a separate sequence of timed transcript
tokens from the ASR system. Rendering aligns each passage's authoritative
restored text to the tokens inside its coarse source range. Consecutive
passages with overlapping provenance are aligned together so repeated local
phrases cannot reorder their boundaries. Adjacent token-derived intervals
share a single boundary. If fewer than 60% of a passage's normalized characters
match exactly, its alignment group falls back to coarse transcript-segment
intervals rather than presenting invented precision. The threshold is an
initial product policy to revisit with more varied lectures.

Playback intervals are derived presentation data. They are not persisted as
part of the semantic analysis artifact and do not change passage provenance,
scores, or slide alignment. A report can therefore be regenerated with better
timing evidence without rerunning restoration or annotation.
