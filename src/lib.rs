mod alignment;
mod analysis_artifact;
mod annotation;
pub mod browser_runtime;
mod chat_completions;
mod codex;
mod comparative_ranking;
mod continuous_report;
mod domain;
pub mod evaluation;
pub mod ingestion;
mod model_backend;
mod model_trace;
mod orchestration;
mod passage_boundaries;
mod passage_projection;
mod playback_timing;
pub mod processing;
mod ranking;
mod report;
mod request_scheduling;
mod restoration;
mod restored_annotation;
mod retrieval;
pub mod runtime_tools;
mod validation;
mod windowing;

pub use alignment::{SlideAlignmentError, infer_slide_positions};
pub use analysis_artifact::{RestoredAnalysisArtifact, RestoredAnalysisArtifactError};
pub use annotation::{
    AnalysisAssemblyError, AnnotationMessage, AnnotationTaskError, AnnotationToolError,
    AnnotationToolSession, SlideEvidence, TranscriptWindowAnalysis, TranscriptWindowTask,
    assemble_window_analyses, build_annotation_tasks, validate_window_analysis,
};
pub use chat_completions::{
    AnnotationDiagnostics, AnnotationResult, ChatCompletionsClient, ChatCompletionsConfig,
    ChatCompletionsConfigError, ChatCompletionsError, RestorationDiagnostics,
    RestoredAnnotationResult, TranscriptWindowRestorationResult,
};
pub use codex::{
    CodexAppServerClient, CodexAppServerConfig, CodexAppServerConfigError,
    CodexModelDiscoveryError, CodexModelInfo, CodexReasoningEffort, CodexServiceTier,
    discover_codex_models,
};
pub use comparative_ranking::{
    ComparativeDecision, ComparativeMetric, ComparativeRankingBatchInfo,
    ComparativeRankingBatchResult, ComparativeRankingConfig, ComparativeRankingConfigError,
    ComparativeRankingError, ComparativeRankingProgress, ComparativeRankingProgressError,
    ComparativeRankingSession, ComparativeRankingTask, ComparativeRankingValidationError,
    ComparativeRankings, CompleteComparativeRanking,
};
pub use continuous_report::{
    ContinuousReportError, ContinuousReportMedia, ReportAudio, ReportSlideImage,
    render_continuous_report, render_continuous_report_with_media,
};
pub use domain::{
    ComparativeScore, ComparativeScoreError, LecturePassage, LecturePassages,
    RestoredLecturePassage, RestoredTranscript, RestoredTranscriptSpan, Score5, ScoreOutOfRange,
    Slide, SlideDeck, SlideId, TimedTranscript, TimedTranscriptToken, Transcript,
    TranscriptSegment, TranscriptSegmentId,
};
pub use model_backend::LectureModelBackend;
pub use model_trace::{
    MODEL_TRACE_FORMAT_VERSION, ModelExchangeTrace, ModelHedgeMetadata, ModelProviderError,
    ModelRequestKind, ModelTraceEvent, ModelTraceRecord, ModelWorkflow, read_model_trace,
};
pub use orchestration::{
    LectureAnalysisConfig, LectureAnalysisConfigError, LectureAnalysisError,
    LectureAnalysisProgress, LectureAnalysisProgressError, LectureAnalysisResult,
    LectureAnalysisSession,
};
pub use passage_boundaries::{
    BOUNDARY_MAX_PASSAGE_CHARACTERS, BoundaryBatchResult, BoundaryBatchTask, BoundaryDecision,
    BoundaryError, BoundarySegmentationPlan, BoundaryWindowDecision, PASSAGE_BOUNDARY_INSTRUCTIONS,
    ProposedBoundaryBatch,
};
pub use passage_projection::{
    PassageProjection, PassageProjectionDiagnostics, PassageProjectionError,
    project_passage_boundaries,
};
pub use playback_timing::{
    PassagePlaybackInterval, PlaybackTimingBasis, PlaybackTimingError,
    project_passage_playback_intervals,
};
pub use ranking::rank_oral_additions;
pub use report::render_report;
pub use request_scheduling::{
    RequestScheduler, RequestSchedulingSnapshot, TokenBudgetError, TokenBudgetSnapshot,
};
pub use restoration::{
    CompleteTranscriptRestoration, RestorationError, RestorationMessage, RestorationProgress,
    RestorationProgressError, RestorationSessionError, TranscriptRestorationConfig,
    TranscriptRestorationConfigError, TranscriptRestorationSession, TranscriptRestorationTask,
    TranscriptWindowRestoration, assemble_restored_transcript, build_restoration_tasks,
    validate_window_restoration,
};
pub use restored_annotation::{
    ProposedLecturePassage, ProposedTranscriptWindowAnalysis, RestoredAnalysisAssemblyError,
    RestoredAnalysisValidationError, RestoredAnnotationError, RestoredAnnotationMessage,
    RestoredTranscriptWindowAnalysis, RestoredTranscriptWindowTask, ValidatedRestoredAnalysis,
    assemble_restored_window_analyses, build_restored_annotation_tasks, project_window_analysis,
    validate_restored_window_analysis,
};
pub use retrieval::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SearchError, SlideScore, SlideScorer,
};
pub use validation::{ValidatedAnalysis, ValidatedSources, ValidationError};
pub use windowing::{
    RestoredTranscriptWindow, RestoredWindowingError, TranscriptWindow, WindowingConfig,
    WindowingConfigError, build_restored_windows, build_windows,
};
