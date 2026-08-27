use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};

use crate::{LecturePassage, LecturePassages, SentenceId, SlideDeck, SlideId, Transcript};

#[derive(Debug)]
pub struct ValidatedAnalysis {
    transcript: Transcript,
    slide_deck: SlideDeck,
    passages: LecturePassages,
}

impl ValidatedAnalysis {
    pub fn new(
        transcript: Transcript,
        slide_deck: SlideDeck,
        passages: LecturePassages,
    ) -> Result<Self, ValidationError> {
        let mut sentence_positions = HashMap::new();
        for (position, sentence) in transcript.sentences.iter().enumerate() {
            if sentence_positions.insert(sentence.id, position).is_some() {
                return Err(ValidationError::DuplicateSentenceId { id: sentence.id });
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

        let mut slide_ids = HashSet::new();
        for slide in &slide_deck.slides {
            if !slide_ids.insert(slide.id) {
                return Err(ValidationError::DuplicateSlideId { id: slide.id });
            }
        }

        for passage in &passages.passages {
            let Some(&start_position) = sentence_positions.get(&passage.start) else {
                return Err(ValidationError::UnknownPassageStart {
                    start: passage.start,
                });
            };
            let Some(&end_position) = sentence_positions.get(&passage.end) else {
                return Err(ValidationError::UnknownPassageEnd {
                    passage_start: passage.start,
                    end: passage.end,
                });
            };
            if end_position < start_position {
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
                if !slide_ids.contains(slide) {
                    return Err(ValidationError::UnknownRelatedSlide {
                        passage_start: passage.start,
                        slide: *slide,
                    });
                }
            }
        }

        let mut next_uncovered = 0;
        for passage in &passages.passages {
            let Some(&start_position) = sentence_positions.get(&passage.start) else {
                return Err(ValidationError::UnknownPassageStart {
                    start: passage.start,
                });
            };
            let Some(&end_position) = sentence_positions.get(&passage.end) else {
                return Err(ValidationError::UnknownPassageEnd {
                    passage_start: passage.start,
                    end: passage.end,
                });
            };
            let Some(expected) = transcript.sentences.get(next_uncovered) else {
                return Err(ValidationError::UnexpectedPassage {
                    actual: passage.start,
                });
            };
            if start_position != next_uncovered {
                return Err(ValidationError::PassageCoverageMismatch {
                    expected: expected.id,
                    actual: passage.start,
                });
            }
            next_uncovered = end_position + 1;
        }
        if let Some(expected) = transcript.sentences.get(next_uncovered) {
            return Err(ValidationError::UncoveredTranscriptTail {
                expected: expected.id,
            });
        }

        Ok(Self {
            transcript,
            slide_deck,
            passages,
        })
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    pub fn slide_deck(&self) -> &SlideDeck {
        &self.slide_deck
    }

    pub fn passages(&self) -> &[LecturePassage] {
        &self.passages.passages
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    DuplicateSentenceId {
        id: SentenceId,
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
    DuplicateSlideId {
        id: SlideId,
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
            Self::DuplicateSentenceId { id } => {
                write!(formatter, "duplicate transcript sentence ID: {}", id.0)
            }
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
            Self::DuplicateSlideId { id } => {
                write!(formatter, "duplicate slide ID: {}", id.0)
            }
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
