mod alignment;
mod annotation;
mod chat_completions;
mod domain;
pub mod evaluation;
pub mod ingestion;
mod model_trace;
mod orchestration;
mod passage_projection;
mod ranking;
mod report;
mod restoration;
mod retrieval;
mod validation;
mod windowing;

pub use alignment::{SlideAlignmentError, infer_slide_positions};
pub use annotation::{
    AnalysisAssemblyError, AnnotationMessage, AnnotationTaskError, AnnotationToolError,
    AnnotationToolSession, SlideEvidence, TranscriptWindowAnalysis, TranscriptWindowTask,
    assemble_window_analyses, build_annotation_tasks, validate_window_analysis,
};
pub use chat_completions::{
    AnnotationDiagnostics, AnnotationResult, ChatCompletionsClient, ChatCompletionsConfig,
    ChatCompletionsConfigError, ChatCompletionsError, RestorationDiagnostics,
    TranscriptWindowRestorationResult,
};
pub use domain::{
    LecturePassage, LecturePassages, RestoredTranscript, RestoredTranscriptSpan, Score5,
    ScoreOutOfRange, Slide, SlideDeck, SlideId, Transcript, TranscriptSegment, TranscriptSegmentId,
};
pub use model_trace::{
    MODEL_TRACE_FORMAT_VERSION, ModelExchangeTrace, ModelProviderError, ModelRequestKind,
    ModelTraceEvent, ModelTraceRecord, ModelWorkflow, read_model_trace,
};
pub use orchestration::{
    LectureAnalysisConfig, LectureAnalysisConfigError, LectureAnalysisError,
    LectureAnalysisProgress, LectureAnalysisProgressError, LectureAnalysisResult,
    LectureAnalysisSession,
};
pub use passage_projection::{
    PassageProjection, PassageProjectionError, project_passage_boundaries,
};
pub use ranking::rank_oral_additions;
pub use report::render_report;
pub use restoration::{
    CompleteTranscriptRestoration, RestorationError, RestorationMessage, RestorationProgress,
    RestorationProgressError, RestorationSessionError, TranscriptRestorationConfig,
    TranscriptRestorationConfigError, TranscriptRestorationSession, TranscriptRestorationTask,
    TranscriptWindowRestoration, assemble_restored_transcript, build_restoration_tasks,
    validate_window_restoration,
};
pub use retrieval::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SearchError, SlideScore, SlideScorer,
};
pub use validation::{ValidatedAnalysis, ValidatedSources, ValidationError};
pub use windowing::{TranscriptWindow, WindowingConfig, WindowingConfigError, build_windows};
