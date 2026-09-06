use std::{error::Error, time::Duration};

use beyond_slides::{
    ChatCompletionsClient, ChatCompletionsConfig, LectureAnalysisConfig,
    LectureAnalysisConfigError, LectureAnalysisError, LectureAnalysisSession, RestoredTranscript,
    RestoredTranscriptSpan, SearchError, Slide, SlideDeck, SlideId, SlideScore, SlideScorer,
    Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources, WindowingConfig,
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

struct FixedScorer;

impl SlideScorer for FixedScorer {
    fn score_slides(&self, _query: &str) -> Result<Vec<SlideScore>, SearchError> {
        Ok(vec![
            SlideScore {
                slide_id: SlideId(0),
                score: 1.0,
            },
            SlideScore {
                slide_id: SlideId(1),
                score: 0.0,
            },
        ])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lecture_analysis_runs_a_canary_then_assembles_every_window() -> Result<(), Box<dyn Error>>
{
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let mut session = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;
    assert!(
        api.received_requests()
            .await
            .expect("mock request recording is enabled")
            .is_empty()
    );

    let canary = session
        .analyze_canary()
        .await?
        .expect("the nonempty transcript has a canary");
    assert_eq!(
        canary.analysis.passages[0].source_start,
        TranscriptSegmentId(0)
    );
    assert_eq!(canary.diagnostics.prompt_tokens, Some(10));
    assert!(session.analyze_canary().await?.is_some());

    let canary_requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(canary_requests.len(), 1);
    assert_eq!(owned_text(&canary_requests[0]), "甲乙");

    let result = session.complete_analysis().await?;

    assert_eq!(result.slide_positions(), [SlideId(0); 3]);
    assert_eq!(result.window_diagnostics().len(), 3);
    assert!(result.window_diagnostics().iter().all(|diagnostics| {
        diagnostics.prompt_tokens == Some(10) && diagnostics.completion_tokens == Some(5)
    }));
    assert_eq!(
        result
            .analysis()
            .passages()
            .iter()
            .map(|passage| (passage.source_start, passage.source_end))
            .collect::<Vec<_>>(),
        vec![
            (TranscriptSegmentId(0), TranscriptSegmentId(0)),
            (TranscriptSegmentId(1), TranscriptSegmentId(1)),
            (TranscriptSegmentId(2), TranscriptSegmentId(2)),
        ]
    );

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 3);
    assert_eq!(owned_text(&requests[0]), "甲乙");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_canary_prevents_later_window_requests() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: true,
    })
    .await;
    let client = client(&api)?;
    let mut session = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;

    let error = session
        .analyze_canary()
        .await
        .expect_err("the invalid first window must stop the lecture run");

    assert!(matches!(
        error,
        LectureAnalysisError::WindowAnnotation {
            window_index: 0,
            ..
        }
    ));
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| owned_text(request) == "甲乙"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lecture_analysis_cannot_complete_before_the_canary() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let session = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;

    let error = session
        .complete_analysis()
        .await
        .expect_err("a nonempty lecture requires a successful canary");

    assert!(matches!(error, LectureAnalysisError::CanaryNotAnalyzed));
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert!(requests.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lecture_analysis_reports_new_windows_in_completion_order() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let mut session = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;
    session.analyze_canary().await?;

    let mut progress = Vec::new();
    session
        .complete_analysis_with_progress(|event| {
            progress.push((
                event.window_index,
                event.completed_windows,
                event.total_windows,
                event.result.analysis.passages[0].source_start,
            ));
            Ok(())
        })
        .await?;

    assert_eq!(
        progress,
        vec![
            (2, 2, 3, TranscriptSegmentId(2)),
            (1, 3, 3, TranscriptSegmentId(1)),
        ]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restored_canary_is_validated_and_not_sent_again() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let mut initial = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;
    let canary = initial
        .analyze_canary()
        .await?
        .expect("the nonempty transcript has a canary")
        .clone();

    let mut invalid = canary.clone();
    invalid.analysis.passages[0].source_start = TranscriptSegmentId(1);
    let mut invalid_resume = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;
    let error = invalid_resume
        .restore_window_result(0, invalid)
        .expect_err("a checkpoint must still partition its original owned region");
    assert!(matches!(
        error,
        LectureAnalysisError::InvalidRestoredWindow { index: 0, .. }
    ));

    let mut resumed = LectureAnalysisSession::prepare(
        &client,
        sources()?,
        restored_transcript(),
        &FixedScorer,
        config(2)?,
    )?;
    assert_eq!(resumed.window_count(), 3);
    assert_eq!(resumed.completed_window_count(), 0);
    resumed.restore_window_result(0, canary)?;
    assert_eq!(resumed.completed_window_count(), 1);
    assert!(resumed.analyze_canary().await?.is_some());
    let result = resumed.complete_analysis().await?;

    assert_eq!(result.window_diagnostics().len(), 3);
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 3);
    assert_eq!(owned_text(&requests[0]), "甲乙");
    let mut resumed_windows = requests[1..].iter().map(owned_text).collect::<Vec<_>>();
    resumed_windows.sort_unstable();
    assert_eq!(resumed_windows, vec!["丙丁", "戊己"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_all_disfluency_restoration_completes_without_model_requests()
-> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let restored = RestoredTranscript {
        spans: vec![RestoredTranscriptSpan::OmittedDisfluency {
            source_start: TranscriptSegmentId(0),
            source_end: TranscriptSegmentId(2),
        }],
    };
    let mut session =
        LectureAnalysisSession::prepare(&client, sources()?, restored, &FixedScorer, config(2)?)?;

    assert_eq!(session.window_count(), 0);
    assert!(session.analyze_canary().await?.is_none());
    let result = session.complete_analysis().await?;

    assert!(result.analysis().passages().is_empty());
    assert!(result.window_diagnostics().is_empty());
    assert!(
        api.received_requests()
            .await
            .expect("mock request recording is enabled")
            .is_empty()
    );
    Ok(())
}

#[test]
fn lecture_analysis_requires_nonzero_concurrency() -> Result<(), Box<dyn Error>> {
    let error = LectureAnalysisConfig::new(windowing()?, 0)
        .expect_err("zero workers cannot process post-canary windows");

    assert_eq!(error, LectureAnalysisConfigError::ZeroConcurrency);
    Ok(())
}

fn client(api: &MockServer) -> Result<ChatCompletionsClient, Box<dyn Error>> {
    Ok(ChatCompletionsClient::new(ChatCompletionsConfig::new(
        format!("{}/v1", api.uri()),
        "test-key",
        "test-model",
    )?))
}

fn config(max_concurrent_windows: usize) -> Result<LectureAnalysisConfig, Box<dyn Error>> {
    Ok(LectureAnalysisConfig::new(
        windowing()?,
        max_concurrent_windows,
    )?)
}

fn windowing() -> Result<WindowingConfig, Box<dyn Error>> {
    Ok(WindowingConfig::new(2, Duration::from_secs(60), 0)?)
}

fn sources() -> Result<ValidatedSources, Box<dyn Error>> {
    Ok(ValidatedSources::new(
        Transcript {
            segments: vec![
                segment(0, 0, 1_000, "甲乙"),
                segment(1, 1_000, 2_000, "丙丁"),
                segment(2, 2_000, 3_000, "戊己"),
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
    )?)
}

fn restored_transcript() -> RestoredTranscript {
    RestoredTranscript {
        spans: vec![
            restored_span(0, "甲乙"),
            restored_span(1, "丙丁"),
            restored_span(2, "戊己"),
        ],
    }
}

fn restored_span(source: u32, text: &str) -> RestoredTranscriptSpan {
    RestoredTranscriptSpan::Text {
        source_start: TranscriptSegmentId(source),
        source_end: TranscriptSegmentId(source),
        text: text.into(),
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

async fn mock_api(responder: WindowAnalysisResponder) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(responder)
        .mount(&server)
        .await;
    server
}

struct WindowAnalysisResponder {
    reject_canary: bool,
}

impl Respond for WindowAnalysisResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let request_body: Value =
            serde_json::from_slice(&request.body).expect("request body is valid JSON");
        let input = request_body["messages"]
            .as_array()
            .expect("messages is an array")
            .iter()
            .find(|message| message["role"] == "user")
            .and_then(|message| message["content"].as_str())
            .expect("request contains user input");
        let task: Value = serde_json::from_str(input).expect("user input is task JSON");
        let owned_text = task["owned_text"].as_str().expect("owned text is a string");
        let window_index = match owned_text {
            "甲乙" => 0,
            "丙丁" => 1,
            "戊己" => 2,
            other => panic!("unexpected owned text {other:?}"),
        };
        let related_slide = if self.reject_canary && window_index == 0 {
            99
        } else {
            0
        };
        let content = json!({
            "passages": [{
                "text": owned_text,
                "connection_strength": 1,
                "related_slides": [related_slide]
            }]
        })
        .to_string();

        let response = ResponseTemplate::new(200).set_body_json(json!({
            "id": format!("chat-{window_index}"),
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": content,
                    "tool_calls": []
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        }));
        if !self.reject_canary && window_index == 1 {
            response.set_delay(Duration::from_millis(50))
        } else {
            response
        }
    }
}

fn owned_text(request: &Request) -> String {
    let request_body: Value = request.body_json().expect("request body is valid JSON");
    let input = request_body["messages"]
        .as_array()
        .expect("messages is an array")
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .expect("request contains user input");
    let task: Value = serde_json::from_str(input).expect("user input is task JSON");
    task["owned_text"]
        .as_str()
        .expect("owned text is a string")
        .to_owned()
}
