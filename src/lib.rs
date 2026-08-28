mod domain;
mod ranking;
mod report;
mod validation;

pub use domain::{
    LecturePassage, LecturePassages, Score5, ScoreOutOfRange, SentenceId, Slide, SlideDeck,
    SlideId, Transcript, TranscriptSentence,
};
pub use ranking::rank_oral_additions;
pub use report::render_report;
pub use validation::{ValidatedAnalysis, ValidationError};
