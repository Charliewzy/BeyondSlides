use std::{error::Error, time::Duration};

use beyond_slides::{
    AnalysisAssemblyError, LexicalSlideScorer, SentenceId, Slide, SlideDeck, SlideId, SlideScorer,
    Transcript, TranscriptSentence, TranscriptWindowAnalysis, ValidatedSources, ValidationError,
    WindowingConfig, assemble_window_analyses, build_windows, infer_slide_positions,
    rank_oral_additions, render_report,
};

#[test]
fn transcript_window_analyses_become_one_validated_analysis() -> Result<(), Box<dyn Error>> {
    let sources = sources();
    let windowing = WindowingConfig::new(4, Duration::from_secs(60), 2)?;
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [
              {
                "start": 2,
                "end": 2,
                "novelty": 3,
                "connection_strength": 2,
                "importance": 5,
                "related_slides": [1],
                "summary": "口头补充"
              },
              {
                "start": 3,
                "end": 3,
                "novelty": 1,
                "connection_strength": 0,
                "importance": 2,
                "related_slides": []
              }
            ]
          }
        ]
        "#,
    )?;

    let analysis = assemble_window_analyses(sources, windowing, window_analyses)?;

    assert_eq!(
        analysis
            .passages()
            .iter()
            .map(|passage| (passage.start, passage.end))
            .collect::<Vec<_>>(),
        vec![
            (SentenceId(0), SentenceId(1)),
            (SentenceId(2), SentenceId(2)),
            (SentenceId(3), SentenceId(3)),
        ]
    );
    Ok(())
}

#[test]
fn transcript_window_analysis_cannot_claim_context_sentences() -> Result<(), Box<dyn Error>> {
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [{
              "start": 1,
              "end": 3,
              "novelty": 3,
              "connection_strength": 2,
              "importance": 5,
              "related_slides": [1]
            }]
          }
        ]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("a transcript window must not claim its left context");

    assert_eq!(
        error,
        AnalysisAssemblyError::PassageOutsideOwnedRegion {
            window: 2,
            owned_start: SentenceId(2),
            owned_end: SentenceId(3),
            passage_start: SentenceId(1),
            passage_end: SentenceId(3),
        }
    );
    Ok(())
}

#[test]
fn transcript_window_analysis_rejects_a_reversed_passage() -> Result<(), Box<dyn Error>> {
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [{
              "start": 3,
              "end": 2,
              "novelty": 3,
              "connection_strength": 2,
              "importance": 5,
              "related_slides": [1]
            }]
          }
        ]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("a lecture passage cannot end before it starts");

    assert_eq!(
        error,
        AnalysisAssemblyError::InvalidAnalysis(ValidationError::PassageEndBeforeStart {
            start: SentenceId(3),
            end: SentenceId(2),
        })
    );
    Ok(())
}

#[test]
fn transcript_window_analysis_cannot_leave_an_owned_sentence_uncovered()
-> Result<(), Box<dyn Error>> {
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [{
              "start": 3,
              "end": 3,
              "novelty": 3,
              "connection_strength": 2,
              "importance": 5,
              "related_slides": [1]
            }]
          }
        ]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("every sentence in an owned region must be covered");

    assert_eq!(
        error,
        AnalysisAssemblyError::WindowCoverageMismatch {
            window: 2,
            expected: SentenceId(2),
            actual: SentenceId(3),
        }
    );
    Ok(())
}

#[test]
fn transcript_window_analysis_cannot_cover_an_owned_sentence_twice() -> Result<(), Box<dyn Error>> {
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [
              {
                "start": 2,
                "end": 3,
                "novelty": 3,
                "connection_strength": 2,
                "importance": 5,
                "related_slides": [1]
              },
              {
                "start": 3,
                "end": 3,
                "novelty": 1,
                "connection_strength": 0,
                "importance": 1,
                "related_slides": []
              }
            ]
          }
        ]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("an owned sentence must not be covered twice");

    assert_eq!(
        error,
        AnalysisAssemblyError::UnexpectedWindowPassage {
            window: 2,
            actual: SentenceId(3),
        }
    );
    Ok(())
}

#[test]
fn transcript_window_analysis_must_reach_the_end_of_its_owned_region() -> Result<(), Box<dyn Error>>
{
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [
          {
            "passages": [{
              "start": 0,
              "end": 1,
              "novelty": 0,
              "connection_strength": 0,
              "importance": 2,
              "related_slides": [0]
            }]
          },
          {
            "passages": [{
              "start": 2,
              "end": 2,
              "novelty": 3,
              "connection_strength": 2,
              "importance": 5,
              "related_slides": [1]
            }]
          }
        ]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("a window response must cover the tail of its owned region");

    assert_eq!(
        error,
        AnalysisAssemblyError::UncoveredWindowTail {
            window: 2,
            expected: SentenceId(3),
        }
    );
    Ok(())
}

#[test]
fn every_transcript_window_requires_exactly_one_analysis() -> Result<(), Box<dyn Error>> {
    let window_analyses: Vec<TranscriptWindowAnalysis> = serde_json::from_str(
        r#"
        [{
          "passages": [{
            "start": 0,
            "end": 1,
            "novelty": 0,
            "connection_strength": 0,
            "importance": 2,
            "related_slides": [0]
          }]
        }]
        "#,
    )?;

    let error = assemble_window_analyses(
        sources(),
        WindowingConfig::new(4, Duration::from_secs(60), 2)?,
        window_analyses,
    )
    .expect_err("one of the two transcript windows has no response");

    assert_eq!(
        error,
        AnalysisAssemblyError::WindowCountMismatch {
            expected: 2,
            actual: 1,
        }
    );
    Ok(())
}

#[test]
fn tiny_course_runs_from_windowing_through_annotation_rendering() -> Result<(), Box<dyn Error>> {
    let transcript = serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))?;
    let slide_deck = serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))?;
    let window_analyses: Vec<TranscriptWindowAnalysis> =
        serde_json::from_str(include_str!("../examples/tiny_course/window-analyses.json"))?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;
    let windowing = WindowingConfig::new(400, Duration::from_secs(12), 150)?;
    let windows = build_windows(&sources, windowing);
    let scorer = LexicalSlideScorer::new(&sources);
    let score_rows = windows
        .iter()
        .map(|window| {
            let query = window
                .owned_region()
                .iter()
                .map(|sentence| sentence.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            scorer.score_slides(&query)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let slide_positions = infer_slide_positions(&score_rows)?;

    assert_eq!(slide_positions.len(), window_analyses.len());
    drop(windows);
    let analysis = assemble_window_analyses(sources, windowing, window_analyses)?;
    let ranked = rank_oral_additions(&analysis);
    let report = render_report(&analysis, &ranked);

    assert_eq!(analysis.passages().len(), 5);
    assert!(report.contains("每次二分查找的比较，都可以理解为获得一比特信息。"));
    assert!(report.contains("按价值排序的口头补充"));
    Ok(())
}

fn sources() -> ValidatedSources {
    ValidatedSources::new(
        Transcript {
            sentences: vec![
                sentence(0, 0, 1_000, "甲乙"),
                sentence(1, 1_000, 2_000, "丙丁"),
                sentence(2, 2_000, 3_000, "戊己"),
                sentence(3, 3_000, 4_000, "庚辛"),
            ],
        },
        SlideDeck {
            slides: vec![
                Slide {
                    id: SlideId(0),
                    text: "第一页".into(),
                },
                Slide {
                    id: SlideId(1),
                    text: "第二页".into(),
                },
            ],
        },
    )
    .expect("the shared sources should be valid")
}

fn sentence(id: u32, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSentence {
    TranscriptSentence {
        id: SentenceId(id),
        start_ms,
        end_ms,
        text: text.into(),
    }
}
