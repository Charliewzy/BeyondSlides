use std::{error::Error, time::Duration};

use beyond_slides::{
    ChatCompletionsClient, ChatCompletionsConfig, LectureAnalysisConfig,
    LectureAnalysisConfigError, LectureAnalysisError, LectureAnalysisSession, SearchError, Slide,
    SlideDeck, SlideId, SlideScore, SlideScorer, Transcript, TranscriptSegment,
    TranscriptSegmentId, ValidatedSources, WindowingConfig,
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
    let mut session =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;
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
    assert_eq!(canary.analysis.passages[0].start, TranscriptSegmentId(0));
    assert_eq!(canary.diagnostics.prompt_tokens, Some(10));
    assert!(session.analyze_canary().await?.is_some());

    let canary_requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(canary_requests.len(), 1);
    assert_eq!(window_number(&canary_requests[0]), 1);

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
            .map(|passage| (passage.start, passage.end))
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
    assert_eq!(window_number(&requests[0]), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_canary_prevents_later_window_requests() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: true,
    })
    .await;
    let client = client(&api)?;
    let mut session =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;

    let error = session
        .analyze_canary()
        .await
        .expect_err("the invalid first window must stop the lecture run");

    assert!(matches!(
        error,
        LectureAnalysisError::WindowAnnotation { window: 1, .. }
    ));
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| window_number(request) == 1));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn lecture_analysis_cannot_complete_before_the_canary() -> Result<(), Box<dyn Error>> {
    let api = mock_api(WindowAnalysisResponder {
        reject_canary: false,
    })
    .await;
    let client = client(&api)?;
    let session = LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;

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
    let mut session =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;
    session.analyze_canary().await?;

    let mut progress = Vec::new();
    session
        .complete_analysis_with_progress(|event| {
            progress.push((
                event.window_number,
                event.completed_windows,
                event.total_windows,
                event.result.analysis.passages[0].start,
            ));
            Ok(())
        })
        .await?;

    assert_eq!(
        progress,
        vec![
            (3, 2, 3, TranscriptSegmentId(2)),
            (2, 3, 3, TranscriptSegmentId(1)),
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
    let mut initial =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;
    let canary = initial
        .analyze_canary()
        .await?
        .expect("the nonempty transcript has a canary")
        .clone();

    let mut invalid = canary.clone();
    invalid.analysis.passages[0].start = TranscriptSegmentId(1);
    let mut invalid_resume =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;
    let error = invalid_resume
        .restore_window_result(0, invalid)
        .expect_err("a checkpoint must still partition its original owned region");
    assert!(matches!(
        error,
        LectureAnalysisError::InvalidRestoredWindow { index: 0, .. }
    ));

    let mut resumed =
        LectureAnalysisSession::prepare(&client, sources()?, &FixedScorer, config(2)?)?;
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
    assert_eq!(window_number(&requests[0]), 1);
    let mut resumed_windows = requests[1..].iter().map(window_number).collect::<Vec<_>>();
    resumed_windows.sort_unstable();
    assert_eq!(resumed_windows, vec![2, 3]);
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

fn segment(id: u32, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms,
        end_ms,
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
        let window_number = task["window_number"]
            .as_u64()
            .expect("window number is an integer");
        let owned_region = task["owned_region"]
            .as_array()
            .expect("owned region is an array");
        let start = owned_region
            .first()
            .and_then(|segment| segment["id"].as_u64())
            .expect("owned region has a first segment");
        let end = owned_region
            .last()
            .and_then(|segment| segment["id"].as_u64())
            .expect("owned region has a last segment");
        let related_slide = if self.reject_canary && window_number == 1 {
            99
        } else {
            0
        };
        let content = json!({
            "passages": [{
                "start": start,
                "end": end,
                "novelty": 2,
                "connection_strength": 1,
                "importance": 3,
                "related_slides": [related_slide]
            }]
        })
        .to_string();

        let response = ResponseTemplate::new(200).set_body_json(json!({
            "id": format!("chat-{window_number}"),
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
        if !self.reject_canary && window_number == 2 {
            response.set_delay(Duration::from_millis(50))
        } else {
            response
        }
    }
}

fn window_number(request: &Request) -> u64 {
    let request_body: Value = request.body_json().expect("request body is valid JSON");
    let input = request_body["messages"]
        .as_array()
        .expect("messages is an array")
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .expect("request contains user input");
    serde_json::from_str::<Value>(input).expect("user input is task JSON")["window_number"]
        .as_u64()
        .expect("window number is an integer")
}
