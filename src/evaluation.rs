mod alignment_visualize;
mod visual_alignment;

pub use alignment_visualize::{
    AlignmentVisualizationError, AlignmentVisualizationWindow, render_alignment_visualization,
};
pub use visual_alignment::{
    FrameSlideMatch, SlideSimilarity, VisualAlignment, VisualAlignmentError,
    VisualAlignmentProgress, align_video_to_slides, align_video_to_slides_with_progress,
};
