mod domain;
mod ranking;
mod report;
mod retrieval;
mod validation;
mod windowing;

pub use domain::{
    LecturePassage, LecturePassages, Score5, ScoreOutOfRange, SentenceId, Slide, SlideDeck,
    SlideId, Transcript, TranscriptSentence,
};
pub use ranking::rank_oral_additions;
pub use report::render_report;
pub use retrieval::{LexicalSlideSearcher, SearchHit, SlideSearcher};
pub use validation::{ValidatedAnalysis, ValidatedSources, ValidationError};
pub use windowing::{TranscriptWindow, WindowingConfig, WindowingConfigError, build_windows};
