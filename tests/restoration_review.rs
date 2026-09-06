use beyond_slides::{
    RestorationDiagnostics, RestoredTranscriptSpan, Transcript, TranscriptSegment,
    TranscriptSegmentId, TranscriptWindowRestoration, TranscriptWindowRestorationResult,
    evaluation::{RestorationReviewError, render_restoration_review},
};

#[test]
fn review_aligns_source_segments_with_restored_spans_and_diagnostics() {
    let transcript = transcript();
    let window_results = vec![
        TranscriptWindowRestorationResult {
            restoration: TranscriptWindowRestoration {
                spans: vec![RestoredTranscriptSpan::Text {
                    source_start: TranscriptSegmentId(0),
                    source_end: TranscriptSegmentId(1),
                    text: "这是经过整理的完整句子。".into(),
                }],
            },
            diagnostics: RestorationDiagnostics {
                provider_retries: 0,
                final_answer_repairs: 1,
                prompt_tokens: Some(100),
                completion_tokens: Some(20),
                accepted_json_fence: false,
            },
        },
        TranscriptWindowRestorationResult {
            restoration: TranscriptWindowRestoration {
                spans: vec![RestoredTranscriptSpan::OmittedDisfluency {
                    source_start: TranscriptSegmentId(2),
                    source_end: TranscriptSegmentId(2),
                }],
            },
            diagnostics: RestorationDiagnostics::default(),
        },
    ];

    let html = render_restoration_review(&transcript, &window_results)
        .expect("valid window checkpoints render");

    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("这是经过整理的完整句子。"));
    assert!(html.contains("data-span-range=\"#0–1\""));
    assert!(html.contains("data-has-repair=\"true\""));
    assert!(html.contains("data-has-omission=\"true\""));
    assert!(html.contains("此范围被明确标记为纯语气词。"));
    assert!(html.contains("只看修复"));
}

#[test]
fn review_rejects_a_gap_between_checkpoint_ranges() {
    let transcript = transcript();
    let results = [TranscriptWindowRestorationResult {
        restoration: TranscriptWindowRestoration {
            spans: vec![RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(1),
                source_end: TranscriptSegmentId(2),
                text: "错误范围".into(),
            }],
        },
        diagnostics: RestorationDiagnostics::default(),
    }];

    let error = render_restoration_review(&transcript, &results)
        .expect_err("review input must cover the transcript from segment zero");

    assert_eq!(
        error,
        RestorationReviewError::CoverageMismatch {
            window_index: 0,
            expected: 0,
            actual: 1,
        }
    );
}

fn transcript() -> Transcript {
    Transcript {
        segments: vec![
            segment(0, 0, 1_000, "这是"),
            segment(1, 1_000, 2_000, "一个句子"),
            segment(2, 2_000, 2_500, "呃"),
        ],
    }
}

fn segment(id: u32, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: Some(start_ms),
        end_ms: Some(end_ms),
        text: text.into(),
    }
}
