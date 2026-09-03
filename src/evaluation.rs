mod alignment_visualize;
mod annotation_quality;
mod model_trace_summary;
mod restoration_review;
mod visual_alignment;

pub use alignment_visualize::{
    AlignmentVisualizationError, AlignmentVisualizationWindow, render_alignment_visualization,
};
pub use annotation_quality::{
    AnnotationQualitySummary, render_annotation_quality, summarize_annotation_quality,
};
pub use model_trace_summary::{
    AffectedTraceWindow, MalformedTraceLine, ModelTraceSummary, WorkflowTraceSummary,
    render_model_trace_summary, summarize_model_trace,
};
pub use restoration_review::{RestorationReviewError, render_restoration_review};
pub use visual_alignment::{
    FrameSlideMatch, SlideSimilarity, VisualAlignment, VisualAlignmentError,
    VisualAlignmentProgress, align_video_to_slides, align_video_to_slides_with_progress,
};
