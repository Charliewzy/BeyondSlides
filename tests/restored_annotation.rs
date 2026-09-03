use std::{error::Error, time::Duration};

use beyond_slides::{
    ProposedTranscriptWindowAnalysis, RestoredAnnotationError, RestoredTranscript,
    RestoredTranscriptSpan, Slide, SlideDeck, SlideId, Transcript, TranscriptSegment,
    TranscriptSegmentId, ValidatedSources, WindowingConfig, assemble_restored_window_analyses,
    build_restored_annotation_tasks, build_restored_windows, project_window_analysis,
};

#[test]
fn annotation_message_contains_readable_text_without_raw_segment_ids() -> Result<(), Box<dyn Error>>
{
    let sources = sources();
    let restored = restored_transcript();
    let windows = build_restored_windows(&sources, &restored, windowing_config())?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(1)])?;

    let message = tasks[0].message()?;
    let input: serde_json::Value = serde_json::from_str(&message.input)?;

    assert_eq!(tasks[0].window_index(), 0);
    assert_eq!(
        input["owned_text"],
        "所有权负责资源管理。借用让函数临时访问数据。"
    );
    assert_eq!(input["left_context"], "");
    assert_eq!(input["right_context"], "");
    assert_eq!(input["slide_position"], 1);
    assert!(input.get("window_index").is_none());
    assert!(!message.input.contains("source_start"));
    assert!(message.instructions.contains("逐字符完全相同"));
    assert!(message.instructions.contains("只允许在原文中选择分界位置"));
    Ok(())
}

#[test]
fn exact_passage_boundaries_can_split_a_coarsely_sourced_restored_span()
-> Result<(), Box<dyn Error>> {
    let sources = sources();
    let restored = restored_transcript();
    let windows = build_restored_windows(&sources, &restored, windowing_config())?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(1)])?;
    let proposed: ProposedTranscriptWindowAnalysis = serde_json::from_value(serde_json::json!({
        "passages": [
            {
                "text": "所有权负责资源管理。",
                "novelty": 1,
                "connection_strength": 2,
                "importance": 4,
                "related_slides": [0]
            },
            {
                "text": "借用让函数临时访问数据。",
                "novelty": 3,
                "connection_strength": 2,
                "importance": 5,
                "related_slides": [1]
            }
        ]
    }))?;

    let (analysis, projection) = project_window_analysis(&sources, &tasks[0], proposed)?;

    assert!(projection.is_exact());
    assert_eq!(analysis.passages.len(), 2);
    assert_eq!(analysis.passages[0].source_start, TranscriptSegmentId(0));
    assert_eq!(analysis.passages[0].source_end, TranscriptSegmentId(1));
    assert_eq!(analysis.passages[1].source_start, TranscriptSegmentId(0));
    assert_eq!(analysis.passages[1].source_end, TranscriptSegmentId(1));
    assert_eq!(
        analysis
            .passages
            .iter()
            .map(|passage| passage.text.as_str())
            .collect::<String>(),
        windows[0].owned_text()
    );
    Ok(())
}

#[test]
fn small_copying_errors_are_replaced_with_authoritative_restored_text() -> Result<(), Box<dyn Error>>
{
    let sources = sources();
    let restored = restored_transcript();
    let windows = build_restored_windows(&sources, &restored, windowing_config())?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(1)])?;
    let proposed: ProposedTranscriptWindowAnalysis = serde_json::from_value(serde_json::json!({
        "passages": [{
            "text": "所有权负责资源管理。借用让函数临时访问据。",
            "novelty": 2,
            "connection_strength": 2,
            "importance": 4,
            "related_slides": [1]
        }]
    }))?;

    let (analysis, projection) = project_window_analysis(&sources, &tasks[0], proposed)?;

    assert_eq!(projection.changed_characters(), 1);
    assert_eq!(analysis.passages[0].text, windows[0].owned_text());
    Ok(())
}

#[test]
fn related_slide_evidence_is_validated_after_text_projection() -> Result<(), Box<dyn Error>> {
    let sources = sources();
    let restored = restored_transcript();
    let windows = build_restored_windows(&sources, &restored, windowing_config())?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(1)])?;
    let proposed: ProposedTranscriptWindowAnalysis = serde_json::from_value(serde_json::json!({
        "passages": [{
            "text": "所有权负责资源管理。借用让函数临时访问数据。",
            "novelty": 2,
            "connection_strength": 2,
            "importance": 4,
            "related_slides": [1, 1]
        }]
    }))?;

    let error = project_window_analysis(&sources, &tasks[0], proposed)
        .expect_err("related slide IDs must remain unique");
    assert_eq!(
        error,
        RestoredAnnotationError::DuplicateRelatedSlide {
            passage_index: 0,
            slide: SlideId(1),
        }
    );
    Ok(())
}

#[test]
fn projected_window_analyses_assemble_into_one_readable_lecture() -> Result<(), Box<dyn Error>> {
    let sources = sources();
    let restored = RestoredTranscript {
        spans: vec![
            RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(0),
                source_end: TranscriptSegmentId(0),
                text: "所有权负责资源管理。".into(),
            },
            RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(1),
                source_end: TranscriptSegmentId(1),
                text: "借用让函数临时访问数据。".into(),
            },
        ],
    };
    let config = WindowingConfig::new(12, Duration::from_secs(60), 0)?;
    let windows = build_restored_windows(&sources, &restored, config)?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(0), SlideId(1)])?;
    let mut analyses = Vec::new();
    for task in &tasks {
        let proposed: ProposedTranscriptWindowAnalysis =
            serde_json::from_value(serde_json::json!({
                "passages": [{
                    "text": task.window().owned_text(),
                    "novelty": 2,
                    "connection_strength": 2,
                    "importance": 4,
                    "related_slides": [task.slide_position()]
                }]
            }))?;
        analyses.push(project_window_analysis(&sources, task, proposed)?.0);
    }

    let analysis = assemble_restored_window_analyses(
        sources,
        restored,
        config,
        &[SlideId(0), SlideId(1)],
        analyses,
    )?;

    assert_eq!(analysis.passages().len(), 2);
    assert_eq!(
        analysis
            .passages()
            .iter()
            .map(|passage| passage.text.as_str())
            .collect::<String>(),
        analysis.restored_transcript().text()
    );
    Ok(())
}

fn sources() -> ValidatedSources {
    let transcript = Transcript {
        segments: vec![segment(0), segment(1)],
    };
    let slides = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(0),
                text: "所有权".into(),
            },
            Slide {
                id: SlideId(1),
                text: "借用".into(),
            },
        ],
    };
    ValidatedSources::new(transcript, slides).expect("valid sources")
}

fn restored_transcript() -> RestoredTranscript {
    RestoredTranscript {
        spans: vec![RestoredTranscriptSpan::Text {
            source_start: TranscriptSegmentId(0),
            source_end: TranscriptSegmentId(1),
            text: "所有权负责资源管理。借用让函数临时访问数据。".into(),
        }],
    }
}

fn segment(id: u32) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: u64::from(id) * 1_000,
        end_ms: u64::from(id + 1) * 1_000,
        text: format!("原始转录{id}"),
    }
}

fn windowing_config() -> WindowingConfig {
    WindowingConfig::new(100, Duration::from_secs(60), 20).expect("valid windowing")
}
