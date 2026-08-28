use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SentenceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SlideId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
pub struct TranscriptSentence {
    pub id: SentenceId,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Transcript {
    pub sentences: Vec<TranscriptSentence>,
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
        self.slides.iter().find(|slide| slide.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LecturePassage {
    pub start: SentenceId,
    pub end: SentenceId,
    pub novelty: Score5,
    pub connection_strength: Score5,
    pub importance: Score5,
    pub related_slides: Vec<SlideId>,
    pub summary: Option<String>,
    pub comparison_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LecturePassages {
    pub passages: Vec<LecturePassage>,
}
