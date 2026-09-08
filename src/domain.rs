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
    pub const ZERO: Self = Self(0);

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

/// Lecture-wide best--worst evidence underlying one displayed score level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparativeScore {
    pub comparisons: u32,
    pub most_selections: u32,
    pub least_selections: u32,
    pub percentile_basis_points: u16,
    pub display_level: Score5,
}

impl ComparativeScore {
    pub fn new(
        comparisons: u32,
        most_selections: u32,
        least_selections: u32,
        percentile_basis_points: u16,
    ) -> Result<Self, ComparativeScoreError> {
        if most_selections
            .checked_add(least_selections)
            .is_none_or(|selections| selections > comparisons)
        {
            return Err(ComparativeScoreError::SelectionsExceedComparisons {
                comparisons,
                most_selections,
                least_selections,
            });
        }
        if percentile_basis_points > 10_000 {
            return Err(ComparativeScoreError::PercentileOutOfRange(
                percentile_basis_points,
            ));
        }
        let display_level = Score5::try_from(
            u8::try_from((percentile_basis_points / 2_000 + 1).min(5))
                .expect("a percentile display level fits in u8"),
        )
        .expect("a percentile display level is between one and five");
        Ok(Self {
            comparisons,
            most_selections,
            least_selections,
            percentile_basis_points,
            display_level,
        })
    }

    pub fn validate(self) -> Result<(), ComparativeScoreError> {
        let expected = Self::new(
            self.comparisons,
            self.most_selections,
            self.least_selections,
            self.percentile_basis_points,
        )?;
        if self.display_level != expected.display_level {
            return Err(ComparativeScoreError::DisplayLevelMismatch {
                percentile_basis_points: self.percentile_basis_points,
                expected: expected.display_level,
                actual: self.display_level,
            });
        }
        Ok(())
    }

    pub fn best_worst_score(self) -> f64 {
        if self.comparisons == 0 {
            return 0.0;
        }
        (f64::from(self.most_selections) - f64::from(self.least_selections))
            / f64::from(self.comparisons)
    }

    pub fn percentile(self) -> f64 {
        f64::from(self.percentile_basis_points) / 100.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparativeScoreError {
    SelectionsExceedComparisons {
        comparisons: u32,
        most_selections: u32,
        least_selections: u32,
    },
    PercentileOutOfRange(u16),
    DisplayLevelMismatch {
        percentile_basis_points: u16,
        expected: Score5,
        actual: Score5,
    },
}

impl fmt::Display for ComparativeScoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelectionsExceedComparisons {
                comparisons,
                most_selections,
                least_selections,
            } => write!(
                formatter,
                "{most_selections} most and {least_selections} least selections exceed {comparisons} comparisons"
            ),
            Self::PercentileOutOfRange(percentile) => write!(
                formatter,
                "comparative percentile {percentile} basis points is outside 0..=10000"
            ),
            Self::DisplayLevelMismatch {
                percentile_basis_points,
                expected,
                actual,
            } => write!(
                formatter,
                "percentile {percentile_basis_points} basis points maps to display level {} rather than {}",
                expected.get(),
                actual.get()
            ),
        }
    }
}

impl Error for ComparativeScoreError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub id: TranscriptSegmentId,
    /// Both timestamps are absent for an untimed source; zero is a real time.
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Transcript {
    #[serde(alias = "sentences")]
    pub segments: Vec<TranscriptSegment>,
}

impl Transcript {
    /// Validated transcripts have timing for every segment or for none of them.
    pub fn has_timestamps(&self) -> bool {
        self.segments
            .first()
            .is_some_and(|segment| segment.start_ms.is_some())
    }
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
    /// Coarse presentation level derived from `comparative_novelty` when present.
    pub novelty: Score5,
    /// Coarse presentation level derived from `comparative_importance` when present.
    pub importance: Score5,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparative_novelty: Option<ComparativeScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparative_importance: Option<ComparativeScore>,
    pub related_slides: Vec<SlideId>,
    pub summary: Option<String>,
    pub comparison_note: Option<String>,
}
