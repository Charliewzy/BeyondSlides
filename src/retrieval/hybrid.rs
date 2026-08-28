use std::collections::HashMap;

use super::{SearchError, SearchHit, SlideSearcher};

const RRF_K: f64 = 60.0;

/// Fuses lexical and dense ranks without comparing their incompatible scores.
pub struct HybridSlideSearcher<'a> {
    lexical: &'a dyn SlideSearcher,
    dense: &'a dyn SlideSearcher,
}

impl<'a> HybridSlideSearcher<'a> {
    pub fn new(lexical: &'a dyn SlideSearcher, dense: &'a dyn SlideSearcher) -> Self {
        Self { lexical, dense }
    }
}

impl SlideSearcher for HybridSlideSearcher<'_> {
    fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, SearchError> {
        if max_results == 0 {
            return Ok(Vec::new());
        }

        let mut fused_scores = HashMap::new();
        for hits in [
            self.lexical.search(query, usize::MAX)?,
            self.dense.search(query, usize::MAX)?,
        ] {
            for (rank, hit) in hits.into_iter().enumerate() {
                *fused_scores.entry(hit.slide_id).or_insert(0.0) +=
                    1.0 / (RRF_K + rank as f64 + 1.0);
            }
        }

        let mut hits: Vec<_> = fused_scores
            .into_iter()
            .map(|(slide_id, score)| SearchHit { slide_id, score })
            .collect();
        hits.sort_by(|left, right| right.score.total_cmp(&left.score));
        hits.truncate(max_results);
        Ok(hits)
    }
}
