mod alignment_visualize;
mod model_trace_summary;
mod visual_alignment;

pub use alignment_visualize::{
    AlignmentVisualizationError, AlignmentVisualizationWindow, render_alignment_visualization,
};
pub use model_trace_summary::{
    AffectedTraceWindow, MalformedTraceLine, ModelTraceSummary, WorkflowTraceSummary,
    render_model_trace_summary, summarize_model_trace,
};
pub use visual_alignment::{
    FrameSlideMatch, SlideSimilarity, VisualAlignment, VisualAlignmentError,
    VisualAlignmentProgress, align_video_to_slides, align_video_to_slides_with_progress,
};
