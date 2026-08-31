mod alignment;
mod annotation;
mod domain;
pub mod evaluation;
pub mod ingestion;
mod orchestration;
mod ranking;
mod report;
mod retrieval;
mod validation;
mod windowing;

pub use alignment::{SlideAlignmentError, infer_slide_positions};
pub use annotation::{
    AnalysisAssemblyError, AnnotationDiagnostics, AnnotationMessage, AnnotationResult,
    AnnotationTaskError, AnnotationToolError, AnnotationToolSession, ChatCompletionsClient,
    ChatCompletionsConfig, ChatCompletionsConfigError, ChatCompletionsError, SlideEvidence,
    TranscriptWindowAnalysis, TranscriptWindowTask, assemble_window_analyses,
    build_annotation_tasks, validate_window_analysis,
};
pub use domain::{
    LecturePassage, LecturePassages, Score5, ScoreOutOfRange, SentenceId, Slide, SlideDeck,
    SlideId, Transcript, TranscriptSentence,
};
pub use orchestration::{
    LectureAnalysisConfig, LectureAnalysisConfigError, LectureAnalysisError,
    LectureAnalysisProgress, LectureAnalysisProgressError, LectureAnalysisResult,
    LectureAnalysisSession,
};
pub use ranking::rank_oral_additions;
pub use report::render_report;
pub use retrieval::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SearchError, SlideScore, SlideScorer,
};
pub use validation::{ValidatedAnalysis, ValidatedSources, ValidationError};
pub use windowing::{TranscriptWindow, WindowingConfig, WindowingConfigError, build_windows};
