use std::{collections::HashSet, error::Error, fmt, ops::Range};

use serde::{Deserialize, Serialize};

use crate::{
    PassageProjection, PassageProjectionError, RestoredLecturePassage, RestoredTranscriptSpan,
    RestoredTranscriptWindow, Score5, Slide, SlideId, ValidatedSources, project_passage_boundaries,
};

const SLIDE_NEIGHBORHOOD_RADIUS: usize = 3;

const ANNOTATION_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/restored_annotation.md"
));

/// The readable transcript and slide evidence for one annotation task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoredTranscriptWindowTask<'a> {
    window_index: usize,
    window: RestoredTranscriptWindow<'a>,
    slide_position: SlideId,
    nearby_slides: &'a [Slide],
}

impl RestoredTranscriptWindowTask<'_> {
    pub const fn window_index(&self) -> usize {
        self.window_index
    }

    pub const fn window(&self) -> RestoredTranscriptWindow<'_> {
        self.window
    }

    pub const fn slide_position(&self) -> SlideId {
        self.slide_position
    }

    pub fn nearby_slides(&self) -> &[Slide] {
        self.nearby_slides
    }

    /// Separates trusted instructions from serialized, untrusted lecture text.
    pub fn message(&self) -> Result<RestoredAnnotationMessage, serde_json::Error> {
        let input = RestoredAnnotationInput {
            left_context: restored_text(self.window.left_context()),
            owned_text: self.window.owned_text(),
            right_context: restored_text(self.window.right_context()),
            slide_position: self.slide_position,
            nearby_slides: self.nearby_slides,
        };
        Ok(RestoredAnnotationMessage {
            instructions: ANNOTATION_INSTRUCTIONS,
            input: serde_json::to_string(&input)?,
        })
    }
}

#[derive(Serialize)]
struct RestoredAnnotationInput<'a> {
    left_context: String,
    owned_text: String,
    right_context: String,
    slide_position: SlideId,
    nearby_slides: &'a [Slide],
}

/// Provider-neutral content for one restored-transcript annotation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredAnnotationMessage {
    pub instructions: &'static str,
    pub input: String,
}

/// One model-proposed passage before its copied text is projected onto source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProposedLecturePassage {
    pub text: String,
    pub novelty: Score5,
    pub connection_strength: Score5,
    pub importance: Score5,
    pub related_slides: Vec<SlideId>,
    pub summary: Option<String>,
    pub comparison_note: Option<String>,
}

/// The untrusted structured response produced for one restored transcript window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProposedTranscriptWindowAnalysis {
    pub passages: Vec<ProposedLecturePassage>,
}

/// A source-backed analysis of one restored transcript window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RestoredTranscriptWindowAnalysis {
    pub passages: Vec<RestoredLecturePassage>,
}

/// Builds one restored-transcript annotation task per inferred slide position.
pub fn build_restored_annotation_tasks<'a>(
    sources: &'a ValidatedSources,
    windows: &[RestoredTranscriptWindow<'a>],
    slide_positions: &[SlideId],
) -> Result<Vec<RestoredTranscriptWindowTask<'a>>, RestoredAnnotationError> {
    if slide_positions.len() != windows.len() {
        return Err(RestoredAnnotationError::SlidePositionCountMismatch {
            expected: windows.len(),
            actual: slide_positions.len(),
        });
    }

    let slide_deck = sources.slide_deck();
    let slides = &slide_deck.slides;
    windows
        .iter()
        .copied()
        .zip(slide_positions.iter().copied())
        .enumerate()
        .map(|(window_index, (window, slide_position))| {
            if window.owned_text().is_empty() {
                return Err(RestoredAnnotationError::EmptyOwnedText { window_index });
            }
            if slide_deck.find(slide_position).is_none() {
                return Err(RestoredAnnotationError::UnknownSlidePosition {
                    window_index,
                    slide: slide_position,
                });
            }

            let position = slide_position.index();
            let neighborhood_start = position.saturating_sub(SLIDE_NEIGHBORHOOD_RADIUS);
            let neighborhood_end = position
                .saturating_add(SLIDE_NEIGHBORHOOD_RADIUS + 1)
                .min(slides.len());
            Ok(RestoredTranscriptWindowTask {
                window_index,
                window,
                slide_position,
                nearby_slides: &slides[neighborhood_start..neighborhood_end],
            })
        })
        .collect()
}

/// Replaces model-copied text with authoritative restored text and attaches provenance.
pub fn project_window_analysis(
    sources: &ValidatedSources,
    task: &RestoredTranscriptWindowTask<'_>,
    proposed: ProposedTranscriptWindowAnalysis,
) -> Result<(RestoredTranscriptWindowAnalysis, PassageProjection), RestoredAnnotationError> {
    let source = task.window.owned_text();
    let projection = project_passage_boundaries(
        &source,
        proposed
            .passages
            .iter()
            .map(|passage| passage.text.as_str()),
    )
    .map_err(RestoredAnnotationError::PassageProjection)?;

    let mut passages = Vec::with_capacity(proposed.passages.len());
    for (index, (proposed, byte_range)) in proposed
        .passages
        .into_iter()
        .zip(projection.byte_ranges())
        .enumerate()
    {
        validate_related_slides(sources, index, &proposed.related_slides)?;
        let (source_start, source_end) = source_provenance(task.window.owned_region(), byte_range)
            .ok_or(RestoredAnnotationError::MissingSourceProvenance {
                passage_index: index,
            })?;
        passages.push(RestoredLecturePassage {
            text: source[byte_range.clone()].to_owned(),
            source_start,
            source_end,
            novelty: proposed.novelty,
            connection_strength: proposed.connection_strength,
            importance: proposed.importance,
            related_slides: proposed.related_slides,
            summary: proposed.summary,
            comparison_note: proposed.comparison_note,
        });
    }

    Ok((RestoredTranscriptWindowAnalysis { passages }, projection))
}

fn validate_related_slides(
    sources: &ValidatedSources,
    passage_index: usize,
    related_slides: &[SlideId],
) -> Result<(), RestoredAnnotationError> {
    let mut unique_slides = HashSet::new();
    for &slide in related_slides {
        if !unique_slides.insert(slide) {
            return Err(RestoredAnnotationError::DuplicateRelatedSlide {
                passage_index,
                slide,
            });
        }
        if sources.slide_deck().find(slide).is_none() {
            return Err(RestoredAnnotationError::UnknownRelatedSlide {
                passage_index,
                slide,
            });
        }
    }
    Ok(())
}

fn source_provenance(
    spans: &[RestoredTranscriptSpan],
    passage_range: &Range<usize>,
) -> Option<(crate::TranscriptSegmentId, crate::TranscriptSegmentId)> {
    let mut text_offset = 0;
    let mut source_start = None;
    let mut source_end = None;

    for span in spans {
        let RestoredTranscriptSpan::Text { text, .. } = span else {
            continue;
        };
        let span_range = text_offset..text_offset + text.len();
        if passage_range.start < span_range.end && passage_range.end > span_range.start {
            source_start.get_or_insert(span.source_start());
            source_end = Some(span.source_end());
        }
        text_offset = span_range.end;
    }

    source_start.zip(source_end)
}

fn restored_text(spans: &[RestoredTranscriptSpan]) -> String {
    spans
        .iter()
        .filter_map(|span| match span {
            RestoredTranscriptSpan::Text { text, .. } => Some(text.as_str()),
            RestoredTranscriptSpan::OmittedDisfluency { .. } => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoredAnnotationError {
    SlidePositionCountMismatch {
        expected: usize,
        actual: usize,
    },
    EmptyOwnedText {
        window_index: usize,
    },
    UnknownSlidePosition {
        window_index: usize,
        slide: SlideId,
    },
    PassageProjection(PassageProjectionError),
    MissingSourceProvenance {
        passage_index: usize,
    },
    UnknownRelatedSlide {
        passage_index: usize,
        slide: SlideId,
    },
    DuplicateRelatedSlide {
        passage_index: usize,
        slide: SlideId,
    },
}

impl fmt::Display for RestoredAnnotationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlidePositionCountMismatch { expected, actual } => write!(
                formatter,
                "annotation expected {expected} inferred slide positions but received {actual}"
            ),
            Self::EmptyOwnedText { window_index } => write!(
                formatter,
                "restored transcript window at index {window_index} contains no readable owned text"
            ),
            Self::UnknownSlidePosition {
                window_index,
                slide,
            } => write!(
                formatter,
                "restored transcript window at index {window_index} has unknown inferred slide position {}",
                slide.0
            ),
            Self::PassageProjection(error) => error.fmt(formatter),
            Self::MissingSourceProvenance { passage_index } => write!(
                formatter,
                "projected lecture passage {passage_index} has no supporting restored transcript span"
            ),
            Self::UnknownRelatedSlide {
                passage_index,
                slide,
            } => write!(
                formatter,
                "projected lecture passage {passage_index} references unknown slide {}",
                slide.0
            ),
            Self::DuplicateRelatedSlide {
                passage_index,
                slide,
            } => write!(
                formatter,
                "projected lecture passage {passage_index} references slide {} more than once",
                slide.0
            ),
        }
    }
}

impl Error for RestoredAnnotationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PassageProjection(error) => Some(error),
            Self::SlidePositionCountMismatch { .. }
            | Self::EmptyOwnedText { .. }
            | Self::UnknownSlidePosition { .. }
            | Self::MissingSourceProvenance { .. }
            | Self::UnknownRelatedSlide { .. }
            | Self::DuplicateRelatedSlide { .. } => None,
        }
    }
}
