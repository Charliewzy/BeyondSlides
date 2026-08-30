use std::{collections::HashSet, error::Error, fmt};

use crate::{LecturePassage, LecturePassages, SentenceId, SlideDeck, SlideId, Transcript};

#[derive(Debug)]
pub struct ValidatedSources {
    transcript: Transcript,
    slide_deck: SlideDeck,
}

impl ValidatedSources {
    pub fn new(transcript: Transcript, slide_deck: SlideDeck) -> Result<Self, ValidationError> {
        for (position, sentence) in transcript.sentences.iter().enumerate() {
            if sentence.id.index() != position {
                return Err(ValidationError::NonCanonicalSentenceId {
                    position,
                    actual: sentence.id,
                });
            }
            if sentence.text.trim().is_empty() {
                return Err(ValidationError::EmptyTranscriptSentence { id: sentence.id });
            }
            if sentence.start_ms > sentence.end_ms {
                return Err(ValidationError::InvalidTranscriptSentenceTimeRange {
                    id: sentence.id,
                    start_ms: sentence.start_ms,
                    end_ms: sentence.end_ms,
                });
            }
        }
        for pair in transcript.sentences.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if current.start_ms < previous.start_ms || current.end_ms < previous.end_ms {
                return Err(ValidationError::TranscriptSentenceOutOfOrder {
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
            let Some(_) = sources.transcript.sentences.get(passage.start.index()) else {
                return Err(ValidationError::UnknownPassageStart {
                    start: passage.start,
                });
            };
            let Some(_) = sources.transcript.sentences.get(passage.end.index()) else {
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
            let Some(expected) = sources.transcript.sentences.get(next_uncovered) else {
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
        if let Some(expected) = sources.transcript.sentences.get(next_uncovered) {
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
    NonCanonicalSentenceId {
        position: usize,
        actual: SentenceId,
    },
    EmptyTranscriptSentence {
        id: SentenceId,
    },
    InvalidTranscriptSentenceTimeRange {
        id: SentenceId,
        start_ms: u64,
        end_ms: u64,
    },
    TranscriptSentenceOutOfOrder {
        previous: SentenceId,
        current: SentenceId,
    },
    NonCanonicalSlideId {
        position: usize,
        actual: SlideId,
    },
    UnknownRelatedSlide {
        passage_start: SentenceId,
        slide: SlideId,
    },
    DuplicateRelatedSlide {
        passage_start: SentenceId,
        slide: SlideId,
    },
    UnknownPassageStart {
        start: SentenceId,
    },
    UnknownPassageEnd {
        passage_start: SentenceId,
        end: SentenceId,
    },
    PassageEndBeforeStart {
        start: SentenceId,
        end: SentenceId,
    },
    PassageCoverageMismatch {
        expected: SentenceId,
        actual: SentenceId,
    },
    UncoveredTranscriptTail {
        expected: SentenceId,
    },
    UnexpectedPassage {
        actual: SentenceId,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalSentenceId { position, actual } => write!(
                formatter,
                "transcript sentence at position {position} must have ID {position}, found {}",
                actual.0
            ),
            Self::EmptyTranscriptSentence { id } => {
                write!(formatter, "transcript sentence {} has empty text", id.0)
            }
            Self::InvalidTranscriptSentenceTimeRange {
                id,
                start_ms,
                end_ms,
            } => write!(
                formatter,
                "transcript sentence {} starts at {} ms but ends at {} ms",
                id.0, start_ms, end_ms
            ),
            Self::TranscriptSentenceOutOfOrder { previous, current } => write!(
                formatter,
                "transcript sentence {} appears before sentence {} in time",
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
                "lecture passage starting at sentence {} references unknown slide {}",
                passage_start.0, slide.0
            ),
            Self::DuplicateRelatedSlide {
                passage_start,
                slide,
            } => write!(
                formatter,
                "lecture passage starting at sentence {} references slide {} more than once",
                passage_start.0, slide.0
            ),
            Self::UnknownPassageStart { start } => write!(
                formatter,
                "lecture passage starts at unknown transcript sentence {}",
                start.0
            ),
            Self::UnknownPassageEnd { passage_start, end } => write!(
                formatter,
                "lecture passage starting at sentence {} ends at unknown transcript sentence {}",
                passage_start.0, end.0
            ),
            Self::PassageEndBeforeStart { start, end } => write!(
                formatter,
                "lecture passage starting at sentence {} ends earlier at sentence {}",
                start.0, end.0
            ),
            Self::PassageCoverageMismatch { expected, actual } => write!(
                formatter,
                "lecture passage coverage expected sentence {} but found sentence {}",
                expected.0, actual.0
            ),
            Self::UncoveredTranscriptTail { expected } => write!(
                formatter,
                "transcript sentence {} and the remaining tail are not covered by a lecture passage",
                expected.0
            ),
            Self::UnexpectedPassage { actual } => write!(
                formatter,
                "lecture passage starting at sentence {} appears after the transcript is already covered",
                actual.0
            ),
        }
    }
}

impl Error for ValidationError {}
