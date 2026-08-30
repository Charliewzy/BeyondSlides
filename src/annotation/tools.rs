use std::{collections::HashSet, error::Error, fmt};

use serde::Serialize;

use super::TranscriptWindowTask;
use crate::{SearchError, Slide, SlideId, SlideScorer, ValidatedSources};

/// Tool state for one transcript window's model conversation.
pub struct AnnotationToolSession<'a> {
    sources: &'a ValidatedSources,
    scorer: &'a dyn SlideScorer,
    visible_slides: HashSet<SlideId>,
}

impl<'a> AnnotationToolSession<'a> {
    /// Starts a session with the task's local slide neighborhood already visible.
    pub fn for_task(
        sources: &'a ValidatedSources,
        scorer: &'a dyn SlideScorer,
        task: &TranscriptWindowTask<'_>,
    ) -> Self {
        Self {
            sources,
            scorer,
            visible_slides: task.nearby_slides.iter().map(|slide| slide.id).collect(),
        }
    }

    /// Returns a slide's text once, then only reports that it is already visible.
    pub fn inspect_slide(
        &mut self,
        slide_id: SlideId,
    ) -> Result<SlideEvidence<'a>, AnnotationToolError> {
        let sources = self.sources;
        let slide = sources
            .slide_deck()
            .find(slide_id)
            .ok_or(AnnotationToolError::UnknownSlide { slide: slide_id })?;
        Ok(self.reveal(slide))
    }

    /// Searches the complete deck without repeating text already in this session.
    ///
    /// The scorer must return one finite score per slide in presentation order. A
    /// zero result limit returns immediately without invoking the scorer.
    pub fn search_slides(
        &mut self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SlideEvidence<'a>>, AnnotationToolError> {
        let slides = ranked_slides(self.sources, self.scorer, query, max_results)?;
        Ok(slides.into_iter().map(|slide| self.reveal(slide)).collect())
    }

    fn reveal(&mut self, slide: &'a Slide) -> SlideEvidence<'a> {
        if self.visible_slides.insert(slide.id) {
            SlideEvidence::Content {
                slide_id: slide.id,
                text: slide.text.as_str(),
            }
        } else {
            SlideEvidence::AlreadyVisible { slide_id: slide.id }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SlideEvidence<'a> {
    Content { slide_id: SlideId, text: &'a str },
    AlreadyVisible { slide_id: SlideId },
}

fn ranked_slides<'a>(
    sources: &'a ValidatedSources,
    scorer: &dyn SlideScorer,
    query: &str,
    max_results: usize,
) -> Result<Vec<&'a Slide>, AnnotationToolError> {
    if max_results == 0 {
        return Ok(Vec::new());
    }

    let scores = scorer
        .score_slides(query)
        .map_err(AnnotationToolError::Search)?;
    let slides = &sources.slide_deck().slides;
    if scores.len() != slides.len() {
        return Err(AnnotationToolError::SlideScoreCountMismatch {
            expected: slides.len(),
            actual: scores.len(),
        });
    }

    for (slide, score) in slides.iter().zip(&scores) {
        if score.slide_id != slide.id {
            return Err(AnnotationToolError::UnexpectedScoredSlide {
                expected: slide.id,
                actual: score.slide_id,
            });
        }
        if !score.score.is_finite() {
            return Err(AnnotationToolError::NonFiniteSlideScore {
                slide: score.slide_id,
            });
        }
    }

    let mut ranked: Vec<_> = slides
        .iter()
        .zip(scores)
        .filter(|(_, score)| score.score > 0.0)
        .map(|(slide, score)| (slide, score.score))
        .collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    ranked.truncate(max_results);
    Ok(ranked.into_iter().map(|(slide, _)| slide).collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationToolError {
    UnknownSlide { slide: SlideId },
    Search(SearchError),
    SlideScoreCountMismatch { expected: usize, actual: usize },
    UnexpectedScoredSlide { expected: SlideId, actual: SlideId },
    NonFiniteSlideScore { slide: SlideId },
}

impl fmt::Display for AnnotationToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSlide { slide } => {
                write!(formatter, "cannot inspect unknown slide {}", slide.0)
            }
            Self::Search(error) => error.fmt(formatter),
            Self::SlideScoreCountMismatch { expected, actual } => write!(
                formatter,
                "slide search expected {expected} scores but received {actual}"
            ),
            Self::UnexpectedScoredSlide { expected, actual } => write!(
                formatter,
                "slide search expected a score for slide {} but received slide {}",
                expected.0, actual.0
            ),
            Self::NonFiniteSlideScore { slide } => write!(
                formatter,
                "slide search received a non-finite score for slide {}",
                slide.0
            ),
        }
    }
}

impl Error for AnnotationToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Search(error) => Some(error),
            Self::UnknownSlide { .. }
            | Self::SlideScoreCountMismatch { .. }
            | Self::UnexpectedScoredSlide { .. }
            | Self::NonFiniteSlideScore { .. } => None,
        }
    }
}
