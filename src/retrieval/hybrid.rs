use std::collections::HashMap;

use super::{SearchError, SlideScore, SlideScorer};

const RRF_K: f64 = 60.0;

/// Fuses lexical and dense ranks without comparing their incompatible scores.
pub struct HybridSlideScorer<'a> {
    lexical: &'a dyn SlideScorer,
    dense: &'a dyn SlideScorer,
}

impl<'a> HybridSlideScorer<'a> {
    pub fn new(lexical: &'a dyn SlideScorer, dense: &'a dyn SlideScorer) -> Self {
        Self { lexical, dense }
    }
}

impl SlideScorer for HybridSlideScorer<'_> {
    fn score_slides(&self, query: &str) -> Result<Vec<SlideScore>, SearchError> {
        let lexical_scores = self.lexical.score_slides(query)?;
        let dense_scores = self.dense.score_slides(query)?;
        if lexical_scores
            .iter()
            .map(|score| score.slide_id)
            .ne(dense_scores.iter().map(|score| score.slide_id))
        {
            return Err(SearchError::IncompatibleSlideScores);
        }

        let mut fused_scores = HashMap::new();
        for scores in [&lexical_scores, &dense_scores] {
            let mut ranked: Vec<_> = scores.iter().filter(|score| score.score > 0.0).collect();
            ranked.sort_by(|left, right| right.score.total_cmp(&left.score));
            let mut rank = 0;
            for position in 0..ranked.len() {
                if position == 0
                    || ranked[position]
                        .score
                        .total_cmp(&ranked[position - 1].score)
                        .is_ne()
                {
                    rank = position + 1;
                }
                *fused_scores.entry(ranked[position].slide_id).or_insert(0.0) +=
                    1.0 / (RRF_K + rank as f64);
            }
        }

        Ok(lexical_scores
            .into_iter()
            .map(|score| SlideScore {
                slide_id: score.slide_id,
                score: fused_scores.get(&score.slide_id).copied().unwrap_or(0.0),
            })
            .collect())
    }
}
