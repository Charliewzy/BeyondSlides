mod chat_completions;
mod tools;

use std::{collections::HashSet, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    LecturePassage, LecturePassages, SentenceId, Slide, SlideId, TranscriptSentence,
    TranscriptWindow, ValidatedAnalysis, ValidatedSources, ValidationError, WindowingConfig,
    build_windows,
};

pub use chat_completions::{
    AnnotationDiagnostics, AnnotationResult, ChatCompletionsClient, ChatCompletionsConfig,
    ChatCompletionsConfigError, ChatCompletionsError,
};
pub use tools::{AnnotationToolError, AnnotationToolSession, SlideEvidence};

const SLIDE_NEIGHBORHOOD_RADIUS: usize = 3;

const ANNOTATION_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/annotation.md"
));

/// The evidence and ownership contract for one agent annotation task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TranscriptWindowTask<'a> {
    pub window_number: usize,
    pub left_context: &'a [TranscriptSentence],
    pub owned_region: &'a [TranscriptSentence],
    pub right_context: &'a [TranscriptSentence],
    pub slide_position: SlideId,
    pub nearby_slides: &'a [Slide],
}

impl TranscriptWindowTask<'_> {
    /// Separates trusted instructions from the serialized, untrusted source input.
    pub fn message(&self) -> Result<AnnotationMessage, serde_json::Error> {
        Ok(AnnotationMessage {
            instructions: ANNOTATION_INSTRUCTIONS,
            input: serde_json::to_string(self)?,
        })
    }
}

/// Provider-neutral content for one model request.
///
/// An adapter should place `instructions` at its highest available instruction
/// priority and supply `input` as user data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationMessage {
    pub instructions: &'static str,
    pub input: String,
}

/// Builds one annotation task per transcript window in presentation order.
pub fn build_annotation_tasks<'a>(
    sources: &'a ValidatedSources,
    windows: &[TranscriptWindow<'a>],
    slide_positions: &[SlideId],
) -> Result<Vec<TranscriptWindowTask<'a>>, AnnotationTaskError> {
    if slide_positions.len() != windows.len() {
        return Err(AnnotationTaskError::SlidePositionCountMismatch {
            expected: windows.len(),
            actual: slide_positions.len(),
        });
    }

    let slide_deck = sources.slide_deck();
    let slides = &slide_deck.slides;
    windows
        .iter()
        .zip(slide_positions.iter().copied())
        .enumerate()
        .map(|(window_index, (window, slide_position))| {
            if slide_deck.find(slide_position).is_none() {
                return Err(AnnotationTaskError::UnknownSlidePosition {
                    window: window_index + 1,
                    slide: slide_position,
                });
            }

            let position = slide_position.index();
            let neighborhood_start = position.saturating_sub(SLIDE_NEIGHBORHOOD_RADIUS);
            let neighborhood_end = position
                .saturating_add(SLIDE_NEIGHBORHOOD_RADIUS + 1)
                .min(slides.len());

            Ok(TranscriptWindowTask {
                window_number: window_index + 1,
                left_context: window.left_context(),
                owned_region: window.owned_region(),
                right_context: window.right_context(),
                slide_position,
                nearby_slides: &slides[neighborhood_start..neighborhood_end],
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationTaskError {
    SlidePositionCountMismatch { expected: usize, actual: usize },
    UnknownSlidePosition { window: usize, slide: SlideId },
}

impl fmt::Display for AnnotationTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlidePositionCountMismatch { expected, actual } => write!(
                formatter,
                "annotation expected {expected} inferred slide positions but received {actual}"
            ),
            Self::UnknownSlidePosition { window, slide } => write!(
                formatter,
                "transcript window {window} has unknown inferred slide position {}",
                slide.0
            ),
        }
    }
}

impl Error for AnnotationTaskError {}

/// The untrusted structured response produced for one transcript window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptWindowAnalysis {
    pub passages: Vec<LecturePassage>,
}

/// Checks one model response against the ownership contract of its task.
///
/// This catches a bad response before other transcript windows are sent. The
/// complete run is still validated again when all window responses are assembled.
pub fn validate_window_analysis(
    sources: &ValidatedSources,
    task: &TranscriptWindowTask<'_>,
    analysis: &TranscriptWindowAnalysis,
) -> Result<(), AnalysisAssemblyError> {
    validate_owned_region(sources, task.window_number, task.owned_region, analysis)
}

/// Assembles one response per transcript window into a validated lecture analysis.
///
/// Responses must be supplied in transcript-window order using the same windowing
/// configuration that created the model tasks. Each response must partition only
/// its owned region; left and right context cannot appear in its lecture passages.
pub fn assemble_window_analyses(
    sources: ValidatedSources,
    windowing_config: WindowingConfig,
    window_analyses: Vec<TranscriptWindowAnalysis>,
) -> Result<ValidatedAnalysis, AnalysisAssemblyError> {
    let passages = {
        let windows = build_windows(&sources, windowing_config);
        let expected_windows = windows.len();
        if window_analyses.len() != expected_windows {
            return Err(AnalysisAssemblyError::WindowCountMismatch {
                expected: expected_windows,
                actual: window_analyses.len(),
            });
        }

        let mut passages = Vec::new();
        for (window_index, (window, analysis)) in windows.iter().zip(window_analyses).enumerate() {
            validate_owned_region(&sources, window_index + 1, window.owned_region(), &analysis)?;
            passages.extend(analysis.passages);
        }

        passages
    };
    ValidatedAnalysis::new(sources, LecturePassages { passages })
        .map_err(AnalysisAssemblyError::InvalidAnalysis)
}

fn validate_owned_region(
    sources: &ValidatedSources,
    window: usize,
    owned_region: &[TranscriptSentence],
    analysis: &TranscriptWindowAnalysis,
) -> Result<(), AnalysisAssemblyError> {
    let Some((owned_start, owned_end)) = owned_region
        .first()
        .zip(owned_region.last())
        .map(|(first, last)| (first.id, last.id))
    else {
        return Err(AnalysisAssemblyError::EmptyOwnedRegion { window });
    };
    let owned_start_position = owned_start.index();
    let owned_end_position = owned_end.index();
    let mut next_uncovered = 0;

    for passage in &analysis.passages {
        let start_position = passage.start.index();
        let end_position = passage.end.index();
        if end_position < start_position {
            return Err(AnalysisAssemblyError::InvalidAnalysis(
                ValidationError::PassageEndBeforeStart {
                    start: passage.start,
                    end: passage.end,
                },
            ));
        }
        if start_position < owned_start_position || end_position > owned_end_position {
            return Err(AnalysisAssemblyError::PassageOutsideOwnedRegion {
                window,
                owned_start,
                owned_end,
                passage_start: passage.start,
                passage_end: passage.end,
            });
        }
        let Some(expected) = owned_region.get(next_uncovered) else {
            return Err(AnalysisAssemblyError::UnexpectedWindowPassage {
                window,
                actual: passage.start,
            });
        };
        if passage.start != expected.id {
            return Err(AnalysisAssemblyError::WindowCoverageMismatch {
                window,
                expected: expected.id,
                actual: passage.start,
            });
        }

        let mut related_slides = HashSet::new();
        for slide in &passage.related_slides {
            if !related_slides.insert(*slide) {
                return Err(AnalysisAssemblyError::InvalidAnalysis(
                    ValidationError::DuplicateRelatedSlide {
                        passage_start: passage.start,
                        slide: *slide,
                    },
                ));
            }
            if sources.slide_deck().find(*slide).is_none() {
                return Err(AnalysisAssemblyError::InvalidAnalysis(
                    ValidationError::UnknownRelatedSlide {
                        passage_start: passage.start,
                        slide: *slide,
                    },
                ));
            }
        }

        next_uncovered = end_position - owned_start_position + 1;
    }

    if let Some(expected) = owned_region.get(next_uncovered) {
        return Err(AnalysisAssemblyError::UncoveredWindowTail {
            window,
            expected: expected.id,
        });
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisAssemblyError {
    WindowCountMismatch {
        expected: usize,
        actual: usize,
    },
    EmptyOwnedRegion {
        window: usize,
    },
    PassageOutsideOwnedRegion {
        window: usize,
        owned_start: SentenceId,
        owned_end: SentenceId,
        passage_start: SentenceId,
        passage_end: SentenceId,
    },
    WindowCoverageMismatch {
        window: usize,
        expected: SentenceId,
        actual: SentenceId,
    },
    UnexpectedWindowPassage {
        window: usize,
        actual: SentenceId,
    },
    UncoveredWindowTail {
        window: usize,
        expected: SentenceId,
    },
    InvalidAnalysis(ValidationError),
}

impl fmt::Display for AnalysisAssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowCountMismatch { expected, actual } => write!(
                formatter,
                "analysis expected {expected} transcript window responses but received {actual}"
            ),
            Self::EmptyOwnedRegion { window } => {
                write!(
                    formatter,
                    "transcript window {window} has an empty owned region"
                )
            }
            Self::PassageOutsideOwnedRegion {
                window,
                owned_start,
                owned_end,
                passage_start,
                passage_end,
            } => write!(
                formatter,
                "transcript window {window} owns sentences {} through {} but contains a lecture passage from sentence {} through {}",
                owned_start.0, owned_end.0, passage_start.0, passage_end.0
            ),
            Self::WindowCoverageMismatch {
                window,
                expected,
                actual,
            } => write!(
                formatter,
                "transcript window {window} expected coverage at sentence {} but found sentence {}",
                expected.0, actual.0
            ),
            Self::UnexpectedWindowPassage { window, actual } => write!(
                formatter,
                "transcript window {window} contains an unexpected lecture passage starting at sentence {} after its owned region is already covered",
                actual.0
            ),
            Self::UncoveredWindowTail { window, expected } => write!(
                formatter,
                "transcript window {window} leaves sentence {} and the remaining owned-region tail uncovered",
                expected.0
            ),
            Self::InvalidAnalysis(error) => error.fmt(formatter),
        }
    }
}

impl Error for AnalysisAssemblyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidAnalysis(error) => Some(error),
            Self::WindowCountMismatch { .. }
            | Self::EmptyOwnedRegion { .. }
            | Self::PassageOutsideOwnedRegion { .. }
            | Self::WindowCoverageMismatch { .. }
            | Self::UnexpectedWindowPassage { .. }
            | Self::UncoveredWindowTail { .. } => None,
        }
    }
}
