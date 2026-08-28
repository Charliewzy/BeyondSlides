use std::sync::Mutex;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use unicode_normalization::UnicodeNormalization;

use crate::{SlideId, ValidatedSources};

use super::{SearchError, SearchHit, SlideSearcher};

const CHINESE_RETRIEVAL_INSTRUCTION: &str = "为这个句子生成表示以用于检索相关文章：";

/// An in-memory dense index using the small BGE Chinese model.
pub struct DenseSlideSearcher {
    model: Mutex<TextEmbedding>,
    slides: Vec<EmbeddedSlide>,
}

impl DenseSlideSearcher {
    /// Loads BAAI/bge-small-zh-v1.5 and embeds every non-empty slide once.
    pub fn try_new(sources: &ValidatedSources) -> Result<Self, SearchError> {
        let mut model = TextEmbedding::try_new(
            TextInitOptions::new(EmbeddingModel::BGESmallZHV15).with_show_download_progress(true),
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
        let slides = indexed_slides
            .into_iter()
            .zip(embeddings)
            .map(|(slide, embedding)| EmbeddedSlide {
                id: slide.id,
                embedding,
            })
            .collect();

        Ok(Self {
            model: Mutex::new(model),
            slides,
        })
    }
}

impl SlideSearcher for DenseSlideSearcher {
    fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, SearchError> {
        if max_results == 0 || query.trim().is_empty() || self.slides.is_empty() {
            return Ok(Vec::new());
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

        let mut hits: Vec<_> = self
            .slides
            .iter()
            .map(|slide| SearchHit {
                slide_id: slide.id,
                score: cosine_similarity(&query_embedding, &slide.embedding),
            })
            .collect();
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits.truncate(max_results);
        Ok(hits)
    }
}

struct EmbeddedSlide {
    id: SlideId,
    embedding: Vec<f32>,
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
