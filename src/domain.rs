use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct TranscriptSegmentId(pub u32);

impl TranscriptSegmentId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SlideId(pub u32);

impl SlideId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "u8")]
pub struct Score5(u8);

impl Score5 {
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Score5 {
    type Error = ScoreOutOfRange;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value <= 5 {
            Ok(Self(value))
        } else {
            Err(ScoreOutOfRange(value))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreOutOfRange(pub u8);

impl fmt::Display for ScoreOutOfRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "score must be between 0 and 5, got {}", self.0)
    }
}

impl Error for ScoreOutOfRange {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub id: TranscriptSegmentId,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Transcript {
    #[serde(alias = "sentences")]
    pub segments: Vec<TranscriptSegment>,
}

/// Fine-grained ASR timing retained separately from transcript segmentation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TimedTranscript {
    pub tokens: Vec<TimedTranscriptToken>,
}

/// One textual ASR unit and its absolute interval in the source recording.
///
/// Chinese units are normally individual characters; Latin units may contain
/// multiple characters and should not be assumed to be linguistic words.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TimedTranscriptToken {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RestoredTranscript {
    pub spans: Vec<RestoredTranscriptSpan>,
}

impl RestoredTranscript {
    pub fn text(&self) -> String {
        self.spans
            .iter()
            .filter_map(|span| match span {
                RestoredTranscriptSpan::Text { text, .. } => Some(text.as_str()),
                RestoredTranscriptSpan::OmittedDisfluency { .. } => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoredTranscriptSpan {
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

impl RestoredTranscriptSpan {
    pub const fn source_start(&self) -> TranscriptSegmentId {
        match self {
            Self::Text { source_start, .. } | Self::OmittedDisfluency { source_start, .. } => {
                *source_start
            }
        }
    }

    pub const fn source_end(&self) -> TranscriptSegmentId {
        match self {
            Self::Text { source_end, .. } | Self::OmittedDisfluency { source_end, .. } => {
                *source_end
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Slide {
    pub id: SlideId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SlideDeck {
    pub slides: Vec<Slide>,
}

impl SlideDeck {
    pub fn find(&self, id: SlideId) -> Option<&Slide> {
        self.slides.get(id.index()).filter(|slide| slide.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LecturePassage {
    pub start: TranscriptSegmentId,
    pub end: TranscriptSegmentId,
    pub novelty: Score5,
    pub connection_strength: Score5,
    pub importance: Score5,
    pub related_slides: Vec<SlideId>,
    pub summary: Option<String>,
    pub comparison_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LecturePassages {
    pub passages: Vec<LecturePassage>,
}

/// A lecture passage over authoritative restored text with raw-source provenance.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RestoredLecturePassage {
    pub text: String,
    /// Earliest transcript segment contributing evidence to this passage.
    /// This is coarse provenance, not an exact playback boundary.
    pub source_start: TranscriptSegmentId,
    /// Latest transcript segment contributing evidence to this passage.
    /// This is coarse provenance, not an exact playback boundary.
    pub source_end: TranscriptSegmentId,
    pub slide_position: SlideId,
    pub novelty: Score5,
    pub connection_strength: Score5,
    pub importance: Score5,
    pub related_slides: Vec<SlideId>,
    pub summary: Option<String>,
    pub comparison_note: Option<String>,
}
