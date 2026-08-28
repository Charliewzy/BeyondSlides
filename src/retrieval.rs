mod dense;
mod hybrid;
mod lexical;

use std::{error::Error, fmt};

use crate::SlideId;

pub use dense::DenseSlideSearcher;
pub use hybrid::HybridSlideSearcher;
pub use lexical::LexicalSlideSearcher;

/// Finds slides by semantic relevance without exposing the retrieval algorithm.
pub trait SlideSearcher {
    fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, SearchError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchHit {
    pub slide_id: SlideId,
    /// Relative relevance within one query's results; larger values rank first.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    ModelInitialization(String),
    Embedding(String),
    ModelUnavailable,
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
        }
    }
}

impl Error for SearchError {}
