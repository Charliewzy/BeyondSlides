use std::error::Error;

use beyond_slides::{
    DenseSlideSearcher, HybridSlideSearcher, LexicalSlideSearcher, SearchHit, Slide, SlideDeck,
    SlideId, SlideSearcher, Transcript, ValidatedSources,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct RetrievalCases {
    queries: Vec<RetrievalCase>,
}

#[derive(Deserialize)]
struct RetrievalCase {
    relevant_slides: Vec<SlideId>,
}

struct FixedSearcher(Vec<SearchHit>);

impl SlideSearcher for FixedSearcher {
    fn search(
        &self,
        _query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, beyond_slides::SearchError> {
        Ok(self.0.iter().copied().take(max_results).collect())
    }
}

fn tiny_course_sources() -> Result<ValidatedSources, Box<dyn Error>> {
    let transcript = serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))?;
    let slide_deck = serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))?;

    Ok(ValidatedSources::new(transcript, slide_deck)?)
}

#[test]
fn lexical_search_matches_chinese_words_across_different_sentences() -> Result<(), Box<dyn Error>> {
    let sources = ValidatedSources::new(
        Transcript { sentences: vec![] },
        SlideDeck {
            slides: vec![
                Slide {
                    id: SlideId(1),
                    text: "学习率控制每次参数更新的步长。".to_owned(),
                },
                Slide {
                    id: SlideId(2),
                    text: "正则化可以缓解模型过拟合。".to_owned(),
                },
            ],
        },
    )?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hits = searcher.search("参数的更新步长由学习率决定", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(1)));
    Ok(())
}

#[test]
fn lexical_search_ranks_strongest_term_evidence_first() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hits = searcher.search("循环不变式 目标值 当前搜索区间", 3)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(2)));
    assert!(hits.windows(2).all(|pair| pair[0].score >= pair[1].score));
    Ok(())
}

#[test]
fn lexical_search_normalizes_case_and_punctuation() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hits = searcher.search("中点 overflow & OFF-BY-ONE!", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(5)));
    Ok(())
}

#[test]
fn lexical_search_normalizes_full_width_technical_terms() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hits = searcher.search("Ｏ（ｌｏｇ ｎ）", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(4)));
    Ok(())
}

#[test]
fn lexical_search_honors_the_result_limit_and_omits_nonmatches() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let searcher = LexicalSlideSearcher::new(&sources);

    assert_eq!(searcher.search("二分查找", 2)?.len(), 2);
    assert!(searcher.search("二分查找", 0)?.is_empty());
    assert!(searcher.search("快速排序 枢轴 partition", 5)?.is_empty());
    assert!(searcher.search("", 5)?.is_empty());
    Ok(())
}

#[test]
fn lexical_search_preserves_english_terms() -> Result<(), Box<dyn Error>> {
    let sources = ValidatedSources::new(
        Transcript { sentences: vec![] },
        SlideDeck {
            slides: vec![
                Slide {
                    id: SlideId(1),
                    text: "Gradient descent updates model parameters.".to_owned(),
                },
                Slide {
                    id: SlideId(2),
                    text: "Regularization reduces overfitting.".to_owned(),
                },
            ],
        },
    )?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hits = searcher.search("GRADIENT descent", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(1)));
    Ok(())
}

#[test]
fn equally_relevant_slides_remain_in_presentation_order() -> Result<(), Box<dyn Error>> {
    let sources = ValidatedSources::new(
        Transcript { sentences: vec![] },
        SlideDeck {
            slides: vec![
                Slide {
                    id: SlideId(20),
                    text: "alpha".to_owned(),
                },
                Slide {
                    id: SlideId(10),
                    text: "alpha".to_owned(),
                },
            ],
        },
    )?;
    let searcher = LexicalSlideSearcher::new(&sources);

    let hit_ids: Vec<_> = searcher
        .search("alpha", 5)?
        .into_iter()
        .map(|hit| hit.slide_id)
        .collect();

    assert_eq!(hit_ids, vec![SlideId(20), SlideId(10)]);
    Ok(())
}

#[test]
fn hybrid_search_rewards_agreement_between_retrieval_modes() -> Result<(), Box<dyn Error>> {
    let lexical = FixedSearcher(vec![
        SearchHit {
            slide_id: SlideId(2),
            score: 8.0,
        },
        SearchHit {
            slide_id: SlideId(1),
            score: 3.0,
        },
    ]);
    let dense = FixedSearcher(vec![
        SearchHit {
            slide_id: SlideId(3),
            score: 0.92,
        },
        SearchHit {
            slide_id: SlideId(1),
            score: 0.88,
        },
    ]);
    let searcher = HybridSlideSearcher::new(&lexical, &dense);

    let hit_ids: Vec<_> = searcher
        .search("共同证据", 3)?
        .into_iter()
        .map(|hit| hit.slide_id)
        .collect();

    assert_eq!(hit_ids.first(), Some(&SlideId(1)));
    assert_eq!(hit_ids.len(), 3);
    assert!(hit_ids.contains(&SlideId(2)));
    assert!(hit_ids.contains(&SlideId(3)));
    Ok(())
}

#[test]
#[ignore = "downloads and runs the Chinese embedding model"]
fn dense_search_matches_a_chinese_semantic_paraphrase() -> Result<(), Box<dyn Error>> {
    let sources = ValidatedSources::new(
        Transcript { sentences: vec![] },
        SlideDeck {
            slides: vec![
                Slide {
                    id: SlideId(1),
                    text: "梯度在深层网络中逐层传播时可能变得极小。".to_owned(),
                },
                Slide {
                    id: SlideId(2),
                    text: "数据增强能够提高训练样本的多样性。".to_owned(),
                },
            ],
        },
    )?;
    let searcher = DenseSlideSearcher::try_new(&sources)?;

    let hits = searcher.search("信号穿过很多层以后几乎衰减没了", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(1)));
    Ok(())
}

#[test]
fn labeled_retrieval_fixture_only_references_existing_slides() -> Result<(), Box<dyn Error>> {
    let slide_deck: SlideDeck =
        serde_json::from_str(include_str!("../examples/retrieval_course/slides.json"))?;
    let cases: RetrievalCases =
        serde_json::from_str(include_str!("../examples/retrieval_course/queries.json"))?;

    assert!(cases.queries.len() >= 12);
    assert!(cases.queries.iter().all(|case| {
        !case.relevant_slides.is_empty()
            && case
                .relevant_slides
                .iter()
                .all(|id| slide_deck.find(*id).is_some())
    }));
    Ok(())
}
