use beyond_slides::{
    BoundarySegmentationPlan, CodexAppServerClient, CodexAppServerConfig, LectureModelBackend,
    RestoredTranscript, RestoredTranscriptSpan, TranscriptRestorationTask, TranscriptSegment,
    TranscriptSegmentId,
};

/// Uses the locally installed Codex CLI and the user's cached `codex login`.
/// It is ignored in normal CI because it consumes subscription quota.
#[tokio::test]
#[ignore = "requires an authenticated local Codex CLI"]
async fn codex_app_server_returns_validated_structured_output()
-> Result<(), Box<dyn std::error::Error>> {
    let restored = RestoredTranscript {
        spans: vec![RestoredTranscriptSpan::Text {
            source_start: TranscriptSegmentId(0),
            source_end: TranscriptSegmentId(0),
            text: "泛型可以复用算法。特型用于表达类型具有的能力。生命周期描述引用之间的关系。"
                .into(),
        }],
    };
    let plan = BoundarySegmentationPlan::new(&restored);
    let task = plan
        .tasks()
        .first()
        .expect("the fixture has candidate gaps");
    let client = CodexAppServerClient::new(CodexAppServerConfig::new("gpt-5.6-luna")?);

    let source = [TranscriptSegment {
        id: TranscriptSegmentId(0),
        start_ms: Some(0),
        end_ms: Some(1_000),
        text: "呃泛型可以复用算法".into(),
    }];
    let restoration = client
        .restore_window(&TranscriptRestorationTask {
            window_index: 0,
            left_context: &[],
            owned_region: &source,
            right_context: &[],
        })
        .await?;
    assert_eq!(restoration.restoration.spans[0].source_start().index(), 0);

    let result = client.classify_passage_boundaries(task).await?;

    task.validate(&result.decisions)?;
    assert_eq!(result.batch_index, 0);
    Ok(())
}
