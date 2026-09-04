use std::{error::Error, time::Duration};

use beyond_slides::{
    ContinuousReportError, ContinuousReportMedia, ProposedTranscriptWindowAnalysis,
    ReportSlideImage, RestoredTranscript, RestoredTranscriptSpan, Slide, SlideDeck, SlideId,
    Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources, WindowingConfig,
    assemble_restored_window_analyses, build_restored_annotation_tasks, build_restored_windows,
    project_window_analysis, render_continuous_report, render_continuous_report_with_media,
};

#[test]
fn report_renders_authoritative_passages_in_continuous_order() -> Result<(), Box<dyn Error>> {
    let analysis = analysis()?;
    let report = render_continuous_report(&analysis);

    let first = report.find("所有权").expect("first passage appears");
    let second = report
        .find("借用让函数临时访问数据。")
        .expect("second passage appears");
    assert!(first < second);
    assert!(report.contains("data-importance=\"4\""));
    assert!(report.contains("data-novelty=\"3\""));
    assert!(report.contains("data-source-start=\"0\""));
    assert!(report.contains("data-time=\"00:00–00:01\""));
    Ok(())
}

#[test]
fn report_distinguishes_aligned_and_currently_viewed_slides() -> Result<(), Box<dyn Error>> {
    let analysis = analysis()?;
    let media = ContinuousReportMedia {
        slide_images: vec![
            ReportSlideImage {
                source: "report.assets/slides/slide-0001.png".into(),
                width: 960,
                height: 540,
            },
            ReportSlideImage {
                source: "report.assets/slides/slide-0002.png".into(),
                width: 960,
                height: 540,
            },
        ],
    };

    let report = render_continuous_report_with_media(&analysis, &media)?;

    assert!(report.contains("data-slide-position=\"0\""));
    assert!(report.contains("data-slide-id=\"0\""));
    assert!(report.contains("data-slide-id=\"1\""));
    assert!(report.contains("src=\"report.assets/slides/slide-0001.png\""));
    assert!(report.contains("slide.classList.toggle(\"aligned\""));
    assert!(report.contains("slide.classList.toggle(\"viewing\""));
    assert!(report.contains("centerSlide(alignedSlide"));
    Ok(())
}

#[test]
fn report_requires_one_image_per_normalized_slide() -> Result<(), Box<dyn Error>> {
    let analysis = analysis()?;
    let media = ContinuousReportMedia {
        slide_images: vec![ReportSlideImage {
            source: "only-one.png".into(),
            width: 960,
            height: 540,
        }],
    };

    let error = render_continuous_report_with_media(&analysis, &media)
        .expect_err("partial slide image collections must be rejected");
    assert_eq!(
        error,
        ContinuousReportError::SlideImageCountMismatch {
            expected: 2,
            actual: 1,
        }
    );
    Ok(())
}

#[test]
fn report_embeds_score_typography_and_escapes_restored_text() -> Result<(), Box<dyn Error>> {
    let analysis = analysis()?;
    assert_eq!(analysis.passages()[0].text, "<所有权 & 资源管理>。");
    let report = render_continuous_report(&analysis);

    assert!(report.contains(".passage[data-importance=\"5\"]"));
    assert!(report.contains(".passage[data-novelty=\"5\"]"));
    assert!(report.contains("text-decoration-thickness"));
    assert!(!report.contains("href=\"http"));
    assert!(!report.contains("src=\"http"));
    assert!(report.contains("所有权"));
    assert!(!report.contains("<所有权 & 资源管理>。"));
    Ok(())
}

fn analysis() -> Result<beyond_slides::ValidatedRestoredAnalysis, Box<dyn Error>> {
    let sources = ValidatedSources::new(
        Transcript {
            segments: vec![segment(0), segment(1)],
        },
        SlideDeck {
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
        },
    )?;
    let restored = RestoredTranscript {
        spans: vec![
            text_span(0, "<所有权 & 资源管理>。"),
            text_span(1, "借用让函数临时访问数据。"),
        ],
    };
    let config = WindowingConfig::new(100, Duration::from_secs(60), 0)?;
    let windows = build_restored_windows(&sources, &restored, config)?;
    let tasks = build_restored_annotation_tasks(&sources, &windows, &[SlideId(0)])?;
    let proposed: ProposedTranscriptWindowAnalysis = serde_json::from_value(serde_json::json!({
        "passages": [
            {
                "text": "<所有权 & 资源管理>。",
                "novelty": 2,
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
    let window_analysis = project_window_analysis(&sources, &tasks[0], proposed)?.0;
    Ok(assemble_restored_window_analyses(
        sources,
        restored,
        config,
        &[SlideId(0)],
        vec![window_analysis],
    )?)
}

fn segment(id: u32) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: u64::from(id) * 1_000,
        end_ms: u64::from(id + 1) * 1_000,
        text: format!("原文{id}"),
    }
}

fn text_span(source: u32, text: &str) -> RestoredTranscriptSpan {
    RestoredTranscriptSpan::Text {
        source_start: TranscriptSegmentId(source),
        source_end: TranscriptSegmentId(source),
        text: text.into(),
    }
}
