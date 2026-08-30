use std::error::Error;

use beyond_slides::{
    Slide, SlideDeck, SlideId, SlideScore,
    evaluation::{
        AlignmentVisualizationWindow, FrameSlideMatch, SlideSimilarity, VisualAlignment,
        render_alignment_visualization,
    },
};

#[test]
fn alignment_visualization_contains_the_score_heatmap_path_and_inspector()
-> Result<(), Box<dyn Error>> {
    let slide_deck = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(0),
                text: "课程标题".into(),
            },
            Slide {
                id: SlideId(1),
                text: "所有权规则".into(),
            },
            Slide {
                id: SlideId(2),
                text: "借用检查".into(),
            },
        ],
    };
    let first_scores = [
        SlideScore {
            slide_id: SlideId(0),
            score: 0.8,
        },
        SlideScore {
            slide_id: SlideId(1),
            score: 0.2,
        },
        SlideScore {
            slide_id: SlideId(2),
            score: 0.1,
        },
    ];
    let second_scores = [
        SlideScore {
            slide_id: SlideId(0),
            score: 0.1,
        },
        SlideScore {
            slide_id: SlideId(1),
            score: 0.9,
        },
        SlideScore {
            slide_id: SlideId(2),
            score: 0.4,
        },
    ];
    let windows = [
        AlignmentVisualizationWindow {
            number: 1,
            start_ms: 0,
            end_ms: 10_000,
            transcript: "大家好，先看课程标题。",
            slide_scores: &first_scores,
            slide_position: SlideId(0),
        },
        AlignmentVisualizationWindow {
            number: 2,
            start_ms: 10_001,
            end_ms: 20_000,
            transcript: "接下来讨论所有权规则。",
            slide_scores: &second_scores,
            slide_position: SlideId(1),
        },
    ];

    let html = render_alignment_visualization(&slide_deck, &windows, None)?;

    assert!(html.contains("data-alignment-heatmap"));
    assert!(html.contains("data-dp-path"));
    assert!(html.contains("data-window-detail=\"2\""));
    assert!(html.contains("接下来讨论所有权规则。"));
    assert!(html.contains("所有权规则"));
    assert!(html.contains("#0d0887"));
    assert!(html.contains("#f0f921"));
    assert!(!html.contains("https://"));
    Ok(())
}

#[test]
fn alignment_visualization_can_overlay_optional_visual_reference_evidence()
-> Result<(), Box<dyn Error>> {
    let slide_deck = SlideDeck {
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
    };
    let scores = [
        SlideScore {
            slide_id: SlideId(0),
            score: 0.8,
        },
        SlideScore {
            slide_id: SlideId(1),
            score: 0.2,
        },
    ];
    let windows = [
        AlignmentVisualizationWindow {
            number: 1,
            start_ms: 0,
            end_ms: 1_000,
            transcript: "视觉参考显示第二页。",
            slide_scores: &scores,
            slide_position: SlideId(0),
        },
        AlignmentVisualizationWindow {
            number: 2,
            start_ms: 2_000,
            end_ms: 3_000,
            transcript: "推断与视觉参考都显示第二页。",
            slide_scores: &scores,
            slide_position: SlideId(1),
        },
    ];
    let visual_alignment = VisualAlignment {
        frame_width: 320,
        frame_height: 180,
        sample_period_ms: 1_000,
        frame_matches: vec![
            FrameSlideMatch {
                timestamp_ms: 0,
                best: SlideSimilarity {
                    slide_id: SlideId(1),
                    score: 0.9,
                },
                runner_up: Some(SlideSimilarity {
                    slide_id: SlideId(0),
                    score: 0.5,
                }),
            },
            FrameSlideMatch {
                timestamp_ms: 2_000,
                best: SlideSimilarity {
                    slide_id: SlideId(1),
                    score: 0.95,
                },
                runner_up: Some(SlideSimilarity {
                    slide_id: SlideId(0),
                    score: 0.4,
                }),
            },
        ],
    };

    let html = render_alignment_visualization(&slide_deck, &windows, Some(&visual_alignment))?;

    assert!(html.contains("data-visual-path"));
    assert!(html.contains("视觉参考 · 幻灯片 2"));
    assert!(html.contains("不一致窗口"));
    assert_eq!(html.matches("data-mismatch-connector").count(), 1);
    Ok(())
}
