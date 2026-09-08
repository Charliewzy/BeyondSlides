use std::{error::Error, time::Duration};

use beyond_slides::{
    AnnotationTaskError, Slide, SlideDeck, SlideId, Transcript, TranscriptSegment,
    TranscriptSegmentId, ValidatedSources, WindowingConfig, build_annotation_tasks, build_windows,
};

#[test]
fn annotation_tasks_preserve_window_ownership_and_slide_neighborhoods() -> Result<(), Box<dyn Error>>
{
    let sources = sources()?;
    let windows = build_windows(&sources, windowing_config()?);
    let tasks = build_annotation_tasks(&sources, &windows, &[SlideId(0), SlideId(4), SlideId(7)])?;

    assert_eq!(tasks.len(), 3);

    assert_eq!(tasks[0].window_number, 1);
    assert!(tasks[0].left_context.is_empty());
    assert_eq!(segment_ids(tasks[0].owned_region), vec![0, 1]);
    assert_eq!(segment_ids(tasks[0].right_context), vec![2]);
    assert_eq!(tasks[0].slide_position, SlideId(0));
    assert_eq!(slide_ids(tasks[0].nearby_slides), vec![0, 1, 2, 3]);

    assert_eq!(tasks[1].window_number, 2);
    assert_eq!(segment_ids(tasks[1].left_context), vec![1]);
    assert_eq!(segment_ids(tasks[1].owned_region), vec![2, 3]);
    assert_eq!(segment_ids(tasks[1].right_context), vec![4]);
    assert_eq!(tasks[1].slide_position, SlideId(4));
    assert_eq!(slide_ids(tasks[1].nearby_slides), vec![1, 2, 3, 4, 5, 6, 7]);

    assert_eq!(tasks[2].window_number, 3);
    assert_eq!(segment_ids(tasks[2].left_context), vec![3]);
    assert_eq!(segment_ids(tasks[2].owned_region), vec![4, 5]);
    assert!(tasks[2].right_context.is_empty());
    assert_eq!(tasks[2].slide_position, SlideId(7));
    assert_eq!(slide_ids(tasks[2].nearby_slides), vec![4, 5, 6, 7]);
    Ok(())
}

#[test]
fn annotation_message_keeps_instructions_separate_from_structured_input()
-> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let windows = build_windows(&sources, windowing_config()?);
    let tasks = build_annotation_tasks(&sources, &windows, &[SlideId(0), SlideId(4), SlideId(7)])?;

    let message = tasks[1].message()?;
    let input: serde_json::Value = serde_json::from_str(&message.input)?;

    assert!(message.instructions.contains("只为 owned_region"));
    assert!(message.instructions.contains("无重叠、无遗漏"));
    assert!(message.instructions.contains("slide_position 只是"));
    assert!(message.instructions.contains("并非对课堂屏幕的观察"));
    assert!(message.instructions.contains("不得将它称为“当前幻灯片”"));
    assert!(message.instructions.contains("nearby_slides 只是"));
    assert!(message.instructions.contains("不代表这些页面当时正在展示"));
    assert!(message.instructions.contains("inspect_slide"));
    assert!(message.instructions.contains("search_slides"));
    assert!(message.instructions.contains("novelty"));
    assert!(message.instructions.contains("importance"));
    assert!(message.instructions.contains("related_slides"));
    assert!(message.instructions.contains("summary 是可选"));
    assert!(message.instructions.contains("comparison_note 是可选"));
    assert!(
        message
            .instructions
            .contains("只返回匹配 TranscriptWindowAnalysis 的 JSON")
    );

    assert_eq!(input["window_number"], 2);
    assert_eq!(json_ids(&input, "left_context"), vec![1]);
    assert_eq!(json_ids(&input, "owned_region"), vec![2, 3]);
    assert_eq!(json_ids(&input, "right_context"), vec![4]);
    assert_eq!(input["slide_position"], 4);
    assert_eq!(json_ids(&input, "nearby_slides"), vec![1, 2, 3, 4, 5, 6, 7]);
    assert!(message.input.contains("幻灯片一"));
    assert!(message.input.contains("幻灯片七"));
    assert!(!message.input.contains("幻灯片零"));
    Ok(())
}

#[test]
fn annotation_tasks_require_one_known_slide_position_per_window() -> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let windows = build_windows(&sources, windowing_config()?);

    let mismatch = build_annotation_tasks(&sources, &windows, &[SlideId(0), SlideId(4)])
        .expect_err("three transcript windows need three inferred slide positions");
    assert_eq!(
        mismatch,
        AnnotationTaskError::SlidePositionCountMismatch {
            expected: 3,
            actual: 2,
        }
    );

    let unknown = build_annotation_tasks(&sources, &windows, &[SlideId(0), SlideId(8), SlideId(7)])
        .expect_err("an inferred position must name an existing slide");
    assert_eq!(
        unknown,
        AnnotationTaskError::UnknownSlidePosition {
            window: 2,
            slide: SlideId(8),
        }
    );
    Ok(())
}

fn sources() -> Result<ValidatedSources, Box<dyn Error>> {
    let transcript = Transcript {
        segments: vec![
            segment(0, "甲甲"),
            segment(1, "乙乙"),
            segment(2, "丙丙"),
            segment(3, "丁丁"),
            segment(4, "戊戊"),
            segment(5, "己己"),
        ],
    };
    let names = ["零", "一", "二", "三", "四", "五", "六", "七"];
    let slide_deck = SlideDeck {
        slides: names
            .into_iter()
            .enumerate()
            .map(|(id, name)| Slide {
                id: SlideId(id as u32),
                text: format!("幻灯片{name}"),
            })
            .collect(),
    };

    Ok(ValidatedSources::new(transcript, slide_deck)?)
}

fn segment(id: u32, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: Some(u64::from(id) * 1_000),
        end_ms: Some(u64::from(id + 1) * 1_000),
        text: text.into(),
    }
}

fn windowing_config() -> Result<WindowingConfig, Box<dyn Error>> {
    Ok(WindowingConfig::new(4, Duration::from_secs(60), 2)?)
}

fn segment_ids(segments: &[TranscriptSegment]) -> Vec<u32> {
    segments.iter().map(|segment| segment.id.0).collect()
}

fn slide_ids(slides: &[Slide]) -> Vec<u32> {
    slides.iter().map(|slide| slide.id.0).collect()
}

fn json_ids(input: &serde_json::Value, field: &str) -> Vec<u64> {
    input[field]
        .as_array()
        .expect("the task field should be an array")
        .iter()
        .map(|item| {
            item["id"]
                .as_u64()
                .expect("every task source should have a numeric ID")
        })
        .collect()
}
