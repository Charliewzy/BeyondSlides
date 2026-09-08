use async_trait::async_trait;

use crate::{
    AnnotationResult, BoundaryBatchResult, BoundaryBatchTask, ChatCompletionsError,
    ComparativeRankingBatchResult, RestoredAnnotationResult, RestoredTranscriptWindowTask,
    SlideScorer, TranscriptRestorationTask, TranscriptWindowRestorationResult,
    TranscriptWindowTask, ValidatedSources, comparative_ranking::ComparativeRankingTask,
};

/// Provider-neutral model operations used by the lecture pipeline.
///
/// A backend receives validated, provider-neutral tasks and must return the
/// same validated results regardless of its wire protocol. Transport details,
/// authentication, structured-output support, and retries stay behind this
/// boundary.
#[async_trait]
pub trait LectureModelBackend: Send + Sync {
    async fn annotate_window(
        &self,
        sources: &ValidatedSources,
        scorer: &dyn SlideScorer,
        task: &TranscriptWindowTask<'_>,
    ) -> Result<AnnotationResult, ChatCompletionsError>;

    async fn annotate_restored_window(
        &self,
        sources: &ValidatedSources,
        scorer: &dyn SlideScorer,
        task: &RestoredTranscriptWindowTask<'_>,
    ) -> Result<RestoredAnnotationResult, ChatCompletionsError>;

    async fn restore_window(
        &self,
        task: &TranscriptRestorationTask<'_>,
    ) -> Result<TranscriptWindowRestorationResult, ChatCompletionsError>;

    async fn compare_passages(
        &self,
        task: &ComparativeRankingTask,
    ) -> Result<ComparativeRankingBatchResult, ChatCompletionsError>;

    async fn classify_passage_boundaries(
        &self,
        task: &BoundaryBatchTask,
    ) -> Result<BoundaryBatchResult, ChatCompletionsError>;
}
