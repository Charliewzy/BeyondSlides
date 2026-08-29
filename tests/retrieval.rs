use std::error::Error;

use beyond_slides::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SearchError, Slide, SlideDeck,
    SlideId, SlideScore, SlideScorer, Transcript, ValidatedSources,
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

struct FixedScorer(Vec<SlideScore>);

impl SlideScorer for FixedScorer {
    fn score_slides(&self, _query: &str) -> Result<Vec<SlideScore>, SearchError> {
        Ok(self.0.clone())
    }
}

fn top_scores(
    scorer: &dyn SlideScorer,
    query: &str,
    max_results: usize,
) -> Result<Vec<SlideScore>, SearchError> {
    let mut scores = scorer.score_slides(query)?;
    scores.sort_by(|left, right| right.score.total_cmp(&left.score));
    scores.truncate(max_results);
    Ok(scores)
}

fn tiny_course_sources() -> Result<ValidatedSources, Box<dyn Error>> {
    let transcript = serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))?;
    let slide_deck = serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))?;

    Ok(ValidatedSources::new(transcript, slide_deck)?)
}

#[test]
fn lexical_scoring_returns_every_slide_in_presentation_order() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let scorer = LexicalSlideScorer::new(&sources);

    let scores = scorer.score_slides("循环不变式")?;

    let expected_ids: Vec<_> = sources
        .slide_deck()
        .slides
        .iter()
        .map(|slide| slide.id)
        .collect();
    assert_eq!(
        scores
            .iter()
            .map(|score| score.slide_id)
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(scores.len(), sources.slide_deck().slides.len());
    assert!(scores.iter().any(|score| score.score == 0.0));
    Ok(())
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
    let scorer = LexicalSlideScorer::new(&sources);

    let hits = top_scores(&scorer, "参数的更新步长由学习率决定", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(1)));
    Ok(())
}

#[test]
fn lexical_search_ranks_strongest_term_evidence_first() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let scorer = LexicalSlideScorer::new(&sources);

    let hits = top_scores(&scorer, "循环不变式 目标值 当前搜索区间", 3)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(2)));
    assert!(hits.windows(2).all(|pair| pair[0].score >= pair[1].score));
    Ok(())
}

#[test]
fn lexical_search_normalizes_case_and_punctuation() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let scorer = LexicalSlideScorer::new(&sources);

    let hits = top_scores(&scorer, "中点 overflow & OFF-BY-ONE!", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(5)));
    Ok(())
}

#[test]
fn lexical_search_normalizes_full_width_technical_terms() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let scorer = LexicalSlideScorer::new(&sources);

    let hits = top_scores(&scorer, "Ｏ（ｌｏｇ ｎ）", 2)?;

    assert_eq!(hits.first().map(|hit| hit.slide_id), Some(SlideId(4)));
    Ok(())
}

#[test]
fn lexical_scoring_uses_zero_for_queries_without_term_evidence() -> Result<(), Box<dyn Error>> {
    let sources = tiny_course_sources()?;
    let scorer = LexicalSlideScorer::new(&sources);

    assert!(
        scorer
            .score_slides("快速排序 枢轴 partition")?
            .iter()
            .all(|score| score.score == 0.0)
    );
    assert!(
        scorer
            .score_slides("")?
            .iter()
            .all(|score| score.score == 0.0)
    );
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
    let scorer = LexicalSlideScorer::new(&sources);

    let hits = top_scores(&scorer, "GRADIENT descent", 2)?;

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
    let scorer = LexicalSlideScorer::new(&sources);

    let hit_ids: Vec<_> = scorer
        .score_slides("alpha")?
        .into_iter()
        .map(|hit| hit.slide_id)
        .collect();

    assert_eq!(hit_ids, vec![SlideId(20), SlideId(10)]);
    Ok(())
}

#[test]
fn hybrid_search_rewards_agreement_between_retrieval_modes() -> Result<(), Box<dyn Error>> {
    let lexical = FixedScorer(vec![
        SlideScore {
            slide_id: SlideId(1),
            score: 3.0,
        },
        SlideScore {
            slide_id: SlideId(2),
            score: 8.0,
        },
        SlideScore {
            slide_id: SlideId(3),
            score: 0.0,
        },
    ]);
    let dense = FixedScorer(vec![
        SlideScore {
            slide_id: SlideId(1),
            score: 0.88,
        },
        SlideScore {
            slide_id: SlideId(2),
            score: 0.0,
        },
        SlideScore {
            slide_id: SlideId(3),
            score: 0.92,
        },
    ]);
    let scorer = HybridSlideScorer::new(&lexical, &dense);

    let scores = scorer.score_slides("共同证据")?;
    assert_eq!(
        scores
            .iter()
            .map(|score| score.slide_id)
            .collect::<Vec<_>>(),
        vec![SlideId(1), SlideId(2), SlideId(3)]
    );
    let hit_ids: Vec<_> = top_scores(&scorer, "共同证据", 3)?
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
    let scorer = DenseSlideScorer::try_new(&sources)?;

    let hits = top_scores(&scorer, "信号穿过很多层以后几乎衰减没了", 2)?;

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
