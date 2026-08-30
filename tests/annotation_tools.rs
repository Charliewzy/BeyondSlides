use std::{error::Error, time::Duration};

use beyond_slides::{
    AnnotationToolError, AnnotationToolSession, SearchError, SentenceId, Slide, SlideDeck,
    SlideEvidence, SlideId, SlideScore, SlideScorer, Transcript, TranscriptSentence,
    TranscriptWindowTask, ValidatedSources, WindowingConfig, build_annotation_tasks, build_windows,
};
use serde_json::json;

struct FixedScorer(Vec<SlideScore>);

impl SlideScorer for FixedScorer {
    fn score_slides(&self, _query: &str) -> Result<Vec<SlideScore>, SearchError> {
        Ok(self.0.clone())
    }
}

struct FailingScorer;

impl SlideScorer for FailingScorer {
    fn score_slides(&self, _query: &str) -> Result<Vec<SlideScore>, SearchError> {
        Err(SearchError::Embedding("test failure".into()))
    }
}

#[test]
fn inspection_returns_each_slide_text_only_once_per_task() -> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));
    let mut session = AnnotationToolSession::for_task(&sources, &scorer, &task);

    assert_eq!(
        session.inspect_slide(SlideId(2))?,
        SlideEvidence::AlreadyVisible {
            slide_id: SlideId(2),
        }
    );
    assert_eq!(
        session.inspect_slide(SlideId(4))?,
        SlideEvidence::Content {
            slide_id: SlideId(4),
            text: "第五张：终止条件",
        }
    );
    assert_eq!(
        session.inspect_slide(SlideId(4))?,
        SlideEvidence::AlreadyVisible {
            slide_id: SlideId(4),
        }
    );
    assert_eq!(
        session
            .inspect_slide(SlideId(6))
            .expect_err("slide 6 does not exist"),
        AnnotationToolError::UnknownSlide { slide: SlideId(6) }
    );
    Ok(())
}

#[test]
fn search_returns_new_text_once_and_marks_repeated_results_as_visible() -> Result<(), Box<dyn Error>>
{
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.25, 0.90, 0.0, 0.1, 0.80, 0.40]));
    let mut session = AnnotationToolSession::for_task(&sources, &scorer, &task);

    let first_results = session.search_slides("循环为什么终止", 3)?;
    assert_eq!(
        serde_json::to_value(first_results)?,
        json!([
            {
                "status": "already_visible",
                "slide_id": 1
            },
            {
                "status": "content",
                "slide_id": 4,
                "text": "第五张：终止条件"
            },
            {
                "status": "content",
                "slide_id": 5,
                "text": "第六张：练习"
            }
        ])
    );

    let repeated_results = session.search_slides("换一种查询", 3)?;
    assert_eq!(
        repeated_results,
        vec![
            SlideEvidence::AlreadyVisible {
                slide_id: SlideId(1),
            },
            SlideEvidence::AlreadyVisible {
                slide_id: SlideId(4),
            },
            SlideEvidence::AlreadyVisible {
                slide_id: SlideId(5),
            },
        ]
    );
    Ok(())
}

#[test]
fn search_omits_zero_scores_and_a_zero_limit_avoids_retrieval() -> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));
    let mut session = AnnotationToolSession::for_task(&sources, &scorer, &task);

    assert!(session.search_slides("没有匹配", 3)?.is_empty());

    let failing_scorer = FailingScorer;
    let mut failing_session = AnnotationToolSession::for_task(&sources, &failing_scorer, &task);
    assert!(failing_session.search_slides("不用执行", 0)?.is_empty());
    Ok(())
}

#[test]
fn search_preserves_retrieval_failures() -> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FailingScorer;
    let mut session = AnnotationToolSession::for_task(&sources, &scorer, &task);

    let error = session
        .search_slides("查询", 3)
        .expect_err("retrieval failure must not look like an empty search result");

    assert_eq!(
        error,
        AnnotationToolError::Search(SearchError::Embedding("test failure".into()))
    );
    assert!(error.source().is_some());
    Ok(())
}

#[test]
fn search_rejects_scores_that_do_not_match_the_validated_slide_deck() -> Result<(), Box<dyn Error>>
{
    let sources = sources()?;
    let task = task(&sources)?;

    let wrong_count = FixedScorer(vec![score(0, 1.0)]);
    let mut session = AnnotationToolSession::for_task(&sources, &wrong_count, &task);
    assert_eq!(
        session
            .search_slides("查询", 3)
            .expect_err("the scorer omitted slides"),
        AnnotationToolError::SlideScoreCountMismatch {
            expected: 6,
            actual: 1,
        }
    );

    let wrong_id = FixedScorer(vec![
        score(0, 1.0),
        score(2, 0.8),
        score(1, 0.7),
        score(3, 0.6),
        score(4, 0.5),
        score(5, 0.4),
    ]);
    let mut session = AnnotationToolSession::for_task(&sources, &wrong_id, &task);
    assert_eq!(
        session
            .search_slides("查询", 3)
            .expect_err("the scorer returned slides out of presentation order"),
        AnnotationToolError::UnexpectedScoredSlide {
            expected: SlideId(1),
            actual: SlideId(2),
        }
    );

    let non_finite = FixedScorer(scores([1.0, f64::NAN, 0.7, 0.6, 0.5, 0.4]));
    let mut session = AnnotationToolSession::for_task(&sources, &non_finite, &task);
    assert_eq!(
        session
            .search_slides("查询", 3)
            .expect_err("tool results require finite retrieval scores"),
        AnnotationToolError::NonFiniteSlideScore { slide: SlideId(1) }
    );
    Ok(())
}

fn sources() -> Result<ValidatedSources, Box<dyn Error>> {
    Ok(ValidatedSources::new(
        Transcript {
            sentences: vec![TranscriptSentence {
                id: SentenceId(0),
                start_ms: 0,
                end_ms: 1_000,
                text: "二分查找的课堂讲解".into(),
            }],
        },
        SlideDeck {
            slides: vec![
                slide(0, "第一张：二分查找"),
                slide(1, "第二张：循环不变式"),
                slide(2, "第三张：边界条件"),
                slide(3, "第四张：复杂度"),
                slide(4, "第五张：终止条件"),
                slide(5, "第六张：练习"),
            ],
        },
    )?)
}

fn task(sources: &ValidatedSources) -> Result<TranscriptWindowTask<'_>, Box<dyn Error>> {
    let windowing_config = WindowingConfig::new(100, Duration::from_secs(60), 20)?;
    let windows = build_windows(sources, windowing_config);
    Ok(build_annotation_tasks(sources, &windows, &[SlideId(0)])?
        .into_iter()
        .next()
        .expect("the transcript creates one annotation task"))
}

fn slide(id: u32, text: &str) -> Slide {
    Slide {
        id: SlideId(id),
        text: text.into(),
    }
}

fn scores(values: [f64; 6]) -> Vec<SlideScore> {
    values
        .into_iter()
        .enumerate()
        .map(|(slide_id, value)| score(slide_id as u32, value))
        .collect()
}

fn score(slide_id: u32, score: f64) -> SlideScore {
    SlideScore {
        slide_id: SlideId(slide_id),
        score,
    }
}
