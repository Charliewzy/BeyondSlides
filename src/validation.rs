use std::{collections::HashSet, error::Error, fmt};

use crate::{LecturePassage, LecturePassages, SlideDeck, SlideId, Transcript, TranscriptSegmentId};

#[derive(Debug)]
pub struct ValidatedSources {
    transcript: Transcript,
    slide_deck: SlideDeck,
}

impl ValidatedSources {
    pub fn new(transcript: Transcript, slide_deck: SlideDeck) -> Result<Self, ValidationError> {
        for (position, segment) in transcript.segments.iter().enumerate() {
            if segment.id.index() != position {
                return Err(ValidationError::NonCanonicalTranscriptSegmentId {
                    position,
                    actual: segment.id,
                });
            }
            if segment.text.trim().is_empty() {
                return Err(ValidationError::EmptyTranscriptSegment { id: segment.id });
            }
            if segment.start_ms > segment.end_ms {
                return Err(ValidationError::InvalidTranscriptSegmentTimeRange {
                    id: segment.id,
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                });
            }
        }
        for pair in transcript.segments.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if current.start_ms < previous.start_ms || current.end_ms < previous.end_ms {
                return Err(ValidationError::TranscriptSegmentOutOfOrder {
                    previous: previous.id,
                    current: current.id,
                });
            }
        }

        for (position, slide) in slide_deck.slides.iter().enumerate() {
            if slide.id.index() != position {
                return Err(ValidationError::NonCanonicalSlideId {
                    position,
                    actual: slide.id,
                });
            }
        }

        Ok(Self {
            transcript,
            slide_deck,
        })
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn slide_deck(&self) -> &SlideDeck {
        &self.slide_deck
    }
}

#[derive(Debug)]
pub struct ValidatedAnalysis {
    sources: ValidatedSources,
    passages: LecturePassages,
}

impl ValidatedAnalysis {
    pub fn new(
        sources: ValidatedSources,
        passages: LecturePassages,
    ) -> Result<Self, ValidationError> {
        let mut next_uncovered = 0;
        for passage in &passages.passages {
            let Some(_) = sources.transcript.segments.get(passage.start.index()) else {
                return Err(ValidationError::UnknownPassageStart {
                    start: passage.start,
                });
            };
            let Some(_) = sources.transcript.segments.get(passage.end.index()) else {
                return Err(ValidationError::UnknownPassageEnd {
                    passage_start: passage.start,
                    end: passage.end,
                });
            };
            if passage.end.index() < passage.start.index() {
                return Err(ValidationError::PassageEndBeforeStart {
                    start: passage.start,
                    end: passage.end,
                });
            }
            let mut related_slides = HashSet::new();
            for slide in &passage.related_slides {
                if !related_slides.insert(*slide) {
                    return Err(ValidationError::DuplicateRelatedSlide {
                        passage_start: passage.start,
                        slide: *slide,
                    });
                }
                if sources.slide_deck.find(*slide).is_none() {
                    return Err(ValidationError::UnknownRelatedSlide {
                        passage_start: passage.start,
                        slide: *slide,
                    });
                }
            }
            let Some(expected) = sources.transcript.segments.get(next_uncovered) else {
                return Err(ValidationError::UnexpectedPassage {
                    actual: passage.start,
                });
            };
            if passage.start != expected.id {
                return Err(ValidationError::PassageCoverageMismatch {
                    expected: expected.id,
                    actual: passage.start,
                });
            }
            next_uncovered = passage.end.index() + 1;
        }
        if let Some(expected) = sources.transcript.segments.get(next_uncovered) {
            return Err(ValidationError::UncoveredTranscriptTail {
                expected: expected.id,
            });
        }

        Ok(Self { sources, passages })
    }

    pub fn transcript(&self) -> &Transcript {
        self.sources.transcript()
    }

    pub fn slide_deck(&self) -> &SlideDeck {
        self.sources.slide_deck()
    }

    pub fn passages(&self) -> &[LecturePassage] {
        &self.passages.passages
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    NonCanonicalTranscriptSegmentId {
        position: usize,
        actual: TranscriptSegmentId,
    },
    EmptyTranscriptSegment {
        id: TranscriptSegmentId,
    },
    InvalidTranscriptSegmentTimeRange {
        id: TranscriptSegmentId,
        start_ms: u64,
        end_ms: u64,
    },
    TranscriptSegmentOutOfOrder {
        previous: TranscriptSegmentId,
        current: TranscriptSegmentId,
    },
    NonCanonicalSlideId {
        position: usize,
        actual: SlideId,
    },
    UnknownRelatedSlide {
        passage_start: TranscriptSegmentId,
        slide: SlideId,
    },
    DuplicateRelatedSlide {
        passage_start: TranscriptSegmentId,
        slide: SlideId,
    },
    UnknownPassageStart {
        start: TranscriptSegmentId,
    },
    UnknownPassageEnd {
        passage_start: TranscriptSegmentId,
        end: TranscriptSegmentId,
    },
    PassageEndBeforeStart {
        start: TranscriptSegmentId,
        end: TranscriptSegmentId,
    },
    PassageCoverageMismatch {
        expected: TranscriptSegmentId,
        actual: TranscriptSegmentId,
    },
    UncoveredTranscriptTail {
        expected: TranscriptSegmentId,
    },
    UnexpectedPassage {
        actual: TranscriptSegmentId,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalTranscriptSegmentId { position, actual } => write!(
                formatter,
                "transcript segment at position {position} must have ID {position}, found {}",
                actual.0
            ),
            Self::EmptyTranscriptSegment { id } => {
                write!(formatter, "transcript segment {} has empty text", id.0)
            }
            Self::InvalidTranscriptSegmentTimeRange {
                id,
                start_ms,
                end_ms,
            } => write!(
                formatter,
                "transcript segment {} starts at {} ms but ends at {} ms",
                id.0, start_ms, end_ms
            ),
            Self::TranscriptSegmentOutOfOrder { previous, current } => write!(
                formatter,
                "transcript segment {} appears before segment {} in time",
                current.0, previous.0
            ),
            Self::NonCanonicalSlideId { position, actual } => write!(
                formatter,
                "slide at position {position} must have ID {position}, found {}",
                actual.0
            ),
            Self::UnknownRelatedSlide {
                passage_start,
                slide,
            } => write!(
                formatter,
                "lecture passage starting at segment {} references unknown slide {}",
                passage_start.0, slide.0
            ),
            Self::DuplicateRelatedSlide {
                passage_start,
                slide,
            } => write!(
                formatter,
                "lecture passage starting at segment {} references slide {} more than once",
                passage_start.0, slide.0
            ),
            Self::UnknownPassageStart { start } => write!(
                formatter,
                "lecture passage starts at unknown transcript segment {}",
                start.0
            ),
            Self::UnknownPassageEnd { passage_start, end } => write!(
                formatter,
                "lecture passage starting at segment {} ends at unknown transcript segment {}",
                passage_start.0, end.0
            ),
            Self::PassageEndBeforeStart { start, end } => write!(
                formatter,
                "lecture passage starting at segment {} ends earlier at segment {}",
                start.0, end.0
            ),
            Self::PassageCoverageMismatch { expected, actual } => write!(
                formatter,
                "lecture passage coverage expected segment {} but found segment {}",
                expected.0, actual.0
            ),
            Self::UncoveredTranscriptTail { expected } => write!(
                formatter,
                "transcript segment {} and the remaining tail are not covered by a lecture passage",
                expected.0
            ),
            Self::UnexpectedPassage { actual } => write!(
                formatter,
                "lecture passage starting at segment {} appears after the transcript is already covered",
                actual.0
            ),
        }
    }
}

impl Error for ValidationError {}
