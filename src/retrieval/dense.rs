use std::sync::Mutex;

use fastembed::{InitOptionsUserDefined, TextEmbedding};
use unicode_normalization::UnicodeNormalization;

use crate::{SlideId, ValidatedSources};

use super::{SearchError, SlideScore, SlideScorer, modelscope::load_embedding_model};

const CHINESE_RETRIEVAL_INSTRUCTION: &str = "为这个句子生成表示以用于检索相关文章：";

/// An in-memory dense index using the small BGE Chinese model.
pub struct DenseSlideScorer {
    model: Mutex<TextEmbedding>,
    slides: Vec<EmbeddedSlide>,
}

impl DenseSlideScorer {
    /// Loads the pinned BAAI/bge-small-zh-v1.5 assets from ModelScope and
    /// embeds every non-empty slide once.
    pub fn try_new(sources: &ValidatedSources) -> Result<Self, SearchError> {
        let user_defined_model = load_embedding_model()
            .map_err(|error| SearchError::ModelInitialization(error.to_string()))?;
        let mut model = TextEmbedding::try_new_from_user_defined(
            user_defined_model,
            InitOptionsUserDefined::default(),
        )
        .map_err(|error| SearchError::ModelInitialization(error.to_string()))?;

        let indexed_slides: Vec<_> = sources
            .slide_deck()
            .slides
            .iter()
            .filter(|slide| !slide.text.trim().is_empty())
            .collect();
        let texts: Vec<_> = indexed_slides
            .iter()
            .map(|slide| normalize(&slide.text))
            .collect();
        let embeddings = if texts.is_empty() {
            Vec::new()
        } else {
            model
                .embed(&texts, None)
                .map_err(|error| SearchError::Embedding(error.to_string()))?
        };
        if embeddings.len() != indexed_slides.len() {
            return Err(SearchError::Embedding(format!(
                "the model returned {} slide vectors for {} slides",
                embeddings.len(),
                indexed_slides.len()
            )));
        }
        let mut embeddings = embeddings.into_iter();
        let slides = sources
            .slide_deck()
            .slides
            .iter()
            .map(|slide| {
                let embedding = if slide.text.trim().is_empty() {
                    None
                } else {
                    Some(embeddings.next().ok_or_else(|| {
                        SearchError::Embedding("a slide vector is missing".into())
                    })?)
                };
                Ok(EmbeddedSlide {
                    id: slide.id,
                    embedding,
                })
            })
            .collect::<Result<Vec<_>, SearchError>>()?;

        Ok(Self {
            model: Mutex::new(model),
            slides,
        })
    }
}

impl SlideScorer for DenseSlideScorer {
    fn score_slides(&self, query: &str) -> Result<Vec<SlideScore>, SearchError> {
        if query.trim().is_empty() || self.slides.is_empty() {
            return Ok(self
                .slides
                .iter()
                .map(|slide| SlideScore {
                    slide_id: slide.id,
                    score: 0.0,
                })
                .collect());
        }

        let instructed_query = format!("{CHINESE_RETRIEVAL_INSTRUCTION}{}", normalize(query));
        let query_embedding = self
            .model
            .lock()
            .map_err(|_| SearchError::ModelUnavailable)?
            .embed([instructed_query], None)
            .map_err(|error| SearchError::Embedding(error.to_string()))?
            .pop()
            .ok_or_else(|| SearchError::Embedding("the model returned no query vector".into()))?;

        Ok(self
            .slides
            .iter()
            .map(|slide| SlideScore {
                slide_id: slide.id,
                score: slide
                    .embedding
                    .as_deref()
                    .map(|embedding| cosine_similarity(&query_embedding, embedding).max(0.0))
                    .unwrap_or(0.0),
            })
            .collect())
    }
}

struct EmbeddedSlide {
    id: SlideId,
    embedding: Option<Vec<f32>>,
}

fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    let (mut dot_product, mut left_norm, mut right_norm) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (&left, &right) in left.iter().zip(right) {
        let left = f64::from(left);
        let right = f64::from(right);
        dot_product += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }

    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot_product / (left_norm.sqrt() * right_norm.sqrt())
    }
}
