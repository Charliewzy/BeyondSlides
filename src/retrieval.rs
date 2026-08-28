use std::collections::{BTreeSet, HashMap};

use jieba_rs::Jieba;
use unicode_normalization::UnicodeNormalization;

use crate::{SlideId, ValidatedSources};

const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// Finds slides by semantic relevance without exposing the retrieval algorithm.
pub trait SlideSearcher {
    fn search(&self, query: &str, max_results: usize) -> Vec<SearchHit>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchHit {
    pub slide_id: SlideId,
    /// Relative relevance within one query's results; larger values rank first.
    pub score: f64,
}

/// An in-memory BM25 index over the validated slide deck.
pub struct LexicalSlideSearcher {
    analyzer: Jieba,
    slides: Vec<IndexedSlide>,
    document_frequencies: HashMap<String, usize>,
    average_document_length: f64,
}

impl LexicalSlideSearcher {
    /// Loads the embedded Jieba dictionary and indexes every slide.
    pub fn new(sources: &ValidatedSources) -> Self {
        let analyzer = Jieba::new();
        let slides: Vec<_> = sources
            .slide_deck()
            .slides
            .iter()
            .map(|slide| IndexedSlide::new(slide.id, &slide.text, &analyzer))
            .collect();

        let mut document_frequencies = HashMap::new();
        for slide in &slides {
            for term in slide.term_frequencies.keys() {
                *document_frequencies.entry(term.clone()).or_insert(0) += 1;
            }
        }

        let total_terms: f64 = slides.iter().map(|slide| slide.term_count as f64).sum();
        let average_document_length = if slides.is_empty() {
            0.0
        } else {
            total_terms / slides.len() as f64
        };

        Self {
            analyzer,
            slides,
            document_frequencies,
            average_document_length,
        }
    }

    fn score_slide(&self, slide: &IndexedSlide, query_terms: &BTreeSet<String>) -> f64 {
        query_terms
            .iter()
            .filter_map(|term| {
                let term_frequency = *slide.term_frequencies.get(term)? as f64;
                let document_frequency = *self.document_frequencies.get(term)? as f64;
                let document_count = self.slides.len() as f64;
                let inverse_document_frequency = (1.0
                    + (document_count - document_frequency + 0.5) / (document_frequency + 0.5))
                    .ln();
                let length_ratio = slide.term_count as f64 / self.average_document_length;
                let saturation = term_frequency * (BM25_K1 + 1.0)
                    / (term_frequency + BM25_K1 * (1.0 - BM25_B + BM25_B * length_ratio));

                Some(inverse_document_frequency * saturation)
            })
            .sum()
    }
}

impl SlideSearcher for LexicalSlideSearcher {
    fn search(&self, query: &str, max_results: usize) -> Vec<SearchHit> {
        if max_results == 0 || self.average_document_length == 0.0 {
            return Vec::new();
        }

        let query_terms: BTreeSet<_> = analyze(query, &self.analyzer).into_iter().collect();
        if query_terms.is_empty() {
            return Vec::new();
        }

        let mut scored_slides: Vec<_> = self
            .slides
            .iter()
            .enumerate()
            .filter_map(|(position, slide)| {
                let score = self.score_slide(slide, &query_terms);
                (score > 0.0).then_some((
                    position,
                    SearchHit {
                        slide_id: slide.id,
                        score,
                    },
                ))
            })
            .collect();

        scored_slides.sort_by(|left, right| {
            right
                .1
                .score
                .total_cmp(&left.1.score)
                .then_with(|| left.0.cmp(&right.0))
        });
        scored_slides.truncate(max_results);
        scored_slides.into_iter().map(|(_, hit)| hit).collect()
    }
}

#[derive(Debug)]
struct IndexedSlide {
    id: SlideId,
    term_count: usize,
    term_frequencies: HashMap<String, usize>,
}

impl IndexedSlide {
    fn new(id: SlideId, text: &str, analyzer: &Jieba) -> Self {
        let terms = analyze(text, analyzer);
        let term_count = terms.len();
        let mut term_frequencies = HashMap::new();
        for term in terms {
            *term_frequencies.entry(term).or_insert(0) += 1;
        }

        Self {
            id,
            term_count,
            term_frequencies,
        }
    }
}

fn analyze(text: &str, analyzer: &Jieba) -> Vec<String> {
    let normalized: String = text.nfkc().collect();
    analyzer
        .cut_for_search(&normalized, true)
        .into_iter()
        .map(|token| token.word)
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .map(str::to_lowercase)
        .collect()
}
