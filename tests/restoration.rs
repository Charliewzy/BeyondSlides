use std::{error::Error, time::Duration};

use beyond_slides::{
    RestorationError, RestoredTranscriptSpan, SlideDeck, Transcript, TranscriptSegment,
    TranscriptSegmentId, TranscriptWindowRestoration, ValidatedSources, WindowingConfig,
    assemble_restored_transcript, build_restoration_tasks, build_windows,
    validate_window_restoration,
};

#[test]
fn restoration_uses_context_across_window_edges_without_claiming_it() -> Result<(), Box<dyn Error>>
{
    let sources = sources(&[
        "所以当我们把",
        "所有权转移时",
        "原来的变量",
        "就不能再使用了",
    ])?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(12, Duration::from_secs(60), 20)?,
    );
    let tasks = build_restoration_tasks(&windows);

    assert_eq!(tasks.len(), 2);
    assert_eq!(segment_ids(tasks[0].owned_region), vec![0, 1]);
    assert_eq!(segment_ids(tasks[0].right_context), vec![2, 3]);
    assert_eq!(segment_ids(tasks[1].left_context), vec![0, 1]);
    assert_eq!(segment_ids(tasks[1].owned_region), vec![2, 3]);

    let message = tasks[0].message()?;
    let input: serde_json::Value = serde_json::from_str(&message.input)?;
    assert!(message.instructions.contains("窗口边缘不代表句子边界"));
    assert!(
        message
            .instructions
            .contains("不要为了让当前窗口看起来完整而强行加句号")
    );
    assert!(
        message
            .instructions
            .contains("默认删除不承载实质含义、仅用于维持语流或寻求附和的确认性口头禅")
    );
    assert!(message.instructions.contains("真正要求听众回答的问题"));
    assert!(input.get("window_index").is_none());
    assert_eq!(
        json_source_ids(&input["output_contract"]["required_source_ids"]),
        vec![0, 1]
    );
    assert_eq!(json_segment_ids(&input, "owned_region"), vec![0, 1]);
    assert_eq!(json_segment_ids(&input, "right_context"), vec![2, 3]);

    let restored = assemble_restored_transcript(
        &windows,
        vec![
            restoration(vec![text_span(0, 1, "所以当我们把所有权转移时，")]),
            restoration(vec![text_span(2, 3, "原来的变量就不能再使用了。")]),
        ],
    )?;

    assert_eq!(
        restored.text(),
        "所以当我们把所有权转移时，原来的变量就不能再使用了。"
    );
    Ok(())
}

#[test]
fn restoration_message_binds_output_to_lecture_global_source_ids() -> Result<(), Box<dyn Error>> {
    let sources = sources(&["甲甲", "乙乙", "丙丙", "丁丁"])?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(4, Duration::from_secs(60), 4)?,
    );
    let tasks = build_restoration_tasks(&windows);
    let message = tasks[1].message()?;
    let input: serde_json::Value = serde_json::from_str(&message.input)?;

    assert_eq!(
        json_source_ids(&input["output_contract"]["required_source_ids"]),
        vec![2, 3]
    );
    assert_eq!(json_segment_ids(&input, "owned_region"), vec![2, 3]);
    assert!(
        message
            .instructions
            .contains("不得从 0 开始按窗口位置重新编号")
    );
    assert!(!message.instructions.contains("\"source_start\": 0"));
    Ok(())
}

#[test]
fn omitted_disfluency_remains_traceable_to_its_source_segment() -> Result<(), Box<dyn Error>> {
    let sources = sources(&["我们来看所有权", "啊", "它解决了资源管理问题"])?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 0)?,
    );
    let tasks = build_restoration_tasks(&windows);
    let response = restoration(vec![
        text_span(0, 0, "我们来看所有权："),
        RestoredTranscriptSpan::OmittedDisfluency {
            source_start: TranscriptSegmentId(1),
            source_end: TranscriptSegmentId(1),
        },
        text_span(2, 2, "它解决了资源管理问题。"),
    ]);

    validate_window_restoration(&tasks[0], &response)?;
    let restored = assemble_restored_transcript(&windows, vec![response])?;

    assert_eq!(restored.text(), "我们来看所有权：它解决了资源管理问题。");
    assert_eq!(
        restored.spans[1],
        RestoredTranscriptSpan::OmittedDisfluency {
            source_start: TranscriptSegmentId(1),
            source_end: TranscriptSegmentId(1),
        }
    );
    Ok(())
}

#[test]
fn malformed_model_output_cannot_skip_owned_segments() -> Result<(), Box<dyn Error>> {
    let sources = sources(&["甲", "乙", "丙"])?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 0)?,
    );
    let tasks = build_restoration_tasks(&windows);
    let response: TranscriptWindowRestoration = serde_json::from_str(
        r#"{
            "spans": [
                {
                    "kind": "text",
                    "source_start": 1,
                    "source_end": 2,
                    "text": "乙丙。"
                }
            ]
        }"#,
    )?;

    let error = validate_window_restoration(&tasks[0], &response)
        .expect_err("a restoration must cover the owned region from its first segment");
    assert_eq!(
        error,
        RestorationError::CoverageMismatch {
            window_index: 0,
            expected: TranscriptSegmentId(0),
            actual: TranscriptSegmentId(1),
        }
    );
    Ok(())
}

#[test]
fn restored_text_cannot_be_blank() -> Result<(), Box<dyn Error>> {
    let sources = sources(&["甲"])?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 0)?,
    );
    let tasks = build_restoration_tasks(&windows);
    let response = restoration(vec![text_span(0, 0, "  ")]);

    let error = validate_window_restoration(&tasks[0], &response)
        .expect_err("blank text must be represented as an explicit omission instead");
    assert_eq!(
        error,
        RestorationError::EmptyText {
            window_index: 0,
            source_start: TranscriptSegmentId(0),
        }
    );
    Ok(())
}

fn sources(texts: &[&str]) -> Result<ValidatedSources, Box<dyn Error>> {
    let segments = texts
        .iter()
        .enumerate()
        .map(|(index, text)| segment(index as u32, text))
        .collect();
    Ok(ValidatedSources::new(
        Transcript { segments },
        SlideDeck { slides: Vec::new() },
    )?)
}

fn segment(id: u32, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: Some(u64::from(id) * 1_000),
        end_ms: Some(u64::from(id + 1) * 1_000),
        text: text.into(),
    }
}

fn restoration(spans: Vec<RestoredTranscriptSpan>) -> TranscriptWindowRestoration {
    TranscriptWindowRestoration { spans }
}

fn text_span(source_start: u32, source_end: u32, text: &str) -> RestoredTranscriptSpan {
    RestoredTranscriptSpan::Text {
        source_start: TranscriptSegmentId(source_start),
        source_end: TranscriptSegmentId(source_end),
        text: text.into(),
    }
}

fn segment_ids(segments: &[TranscriptSegment]) -> Vec<u32> {
    segments.iter().map(|segment| segment.id.0).collect()
}

fn json_segment_ids(input: &serde_json::Value, field: &str) -> Vec<u64> {
    input[field]
        .as_array()
        .expect("the task field should be an array")
        .iter()
        .map(|item| {
            item["id"]
                .as_u64()
                .expect("every transcript segment should have a numeric ID")
        })
        .collect()
}

fn json_source_ids(input: &serde_json::Value) -> Vec<u64> {
    input
        .as_array()
        .expect("source IDs should be an array")
        .iter()
        .map(|id| id.as_u64().expect("every source ID should be numeric"))
        .collect()
}
