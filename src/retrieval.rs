mod dense;
mod hybrid;
mod lexical;

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::SlideId;

pub use dense::DenseSlideScorer;
pub use hybrid::HybridSlideScorer;
pub use lexical::LexicalSlideScorer;

/// Scores every slide for semantic relevance without exposing the retrieval algorithm.
pub trait SlideScorer {
    /// Returns exactly one score per indexed slide in presentation order.
    fn score_slides(&self, query: &str) -> Result<Vec<SlideScore>, SearchError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct SlideScore {
    pub slide_id: SlideId,
    /// Relative relevance within one query; larger values are more relevant.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    ModelInitialization(String),
    Embedding(String),
    ModelUnavailable,
    IncompatibleSlideScores,
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelInitialization(message) => {
                write!(
                    formatter,
                    "could not initialize the embedding model: {message}"
                )
            }
            Self::Embedding(message) => write!(formatter, "could not embed text: {message}"),
            Self::ModelUnavailable => write!(formatter, "the embedding model is unavailable"),
            Self::IncompatibleSlideScores => write!(
                formatter,
                "slide scorers returned different slides or presentation orders"
            ),
        }
    }
}

impl Error for SearchError {}
