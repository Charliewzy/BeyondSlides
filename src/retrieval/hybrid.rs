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

        let mut fused_scores = vec![0.0; lexical_scores.len()];
        for scores in [&lexical_scores, &dense_scores] {
            let mut ranked: Vec<_> = scores
                .iter()
                .enumerate()
                .filter(|(_, score)| score.score > 0.0)
                .collect();
            ranked.sort_by(|left, right| right.1.score.total_cmp(&left.1.score));

            let mut previous_score = None;
            let mut rank = 0;
            for (ranked_position, (slide_position, score)) in ranked.into_iter().enumerate() {
                if previous_score.is_none_or(|previous| previous != score.score) {
                    rank = ranked_position + 1;
                    previous_score = Some(score.score);
                }

                fused_scores[slide_position] += 1.0 / (RRF_K + rank as f64);
            }
        }

        Ok(lexical_scores
            .into_iter()
            .zip(fused_scores)
            .map(|(score, fused_score)| SlideScore {
                slide_id: score.slide_id,
                score: fused_score,
            })
            .collect())
    }
}
