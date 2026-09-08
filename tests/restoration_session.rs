use std::{error::Error, time::Duration};

use beyond_slides::{
    ChatCompletionsClient, ChatCompletionsConfig, RestorationSessionError, Transcript,
    TranscriptRestorationConfig, TranscriptRestorationSession, TranscriptSegment,
    TranscriptSegmentId, WindowingConfig,
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

#[tokio::test(flavor = "multi_thread")]
async fn stopped_restoration_resumes_only_missing_windows() -> Result<(), Box<dyn Error>> {
    use beyond_slides::processing::StopSignal;
    let api = mock_api().await;
    let client = client(&api)?;
    let stop = StopSignal::default();
    let session = TranscriptRestorationSession::prepare(&client, transcript(), config(1)?)?
        .with_stop_signal(stop.clone());
    let mut saved = Vec::new();
    let result = session
        .complete_restoration_with_progress(|event| {
            saved.push((event.window_index, event.result.clone()));
            stop.request_stop();
            Ok(())
        })
        .await;
    assert!(matches!(result, Err(RestorationSessionError::Stopped)));
    assert_eq!(
        saved.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [0]
    );
    assert_eq!(api.received_requests().await.unwrap().len(), 1);

    let mut resumed = TranscriptRestorationSession::prepare(&client, transcript(), config(1)?)?;
    for (index, result) in saved {
        resumed.restore_window_checkpoint(index, result)?;
    }
    let result = resumed.complete_restoration().await?;
    assert_eq!(result.transcript().text(), "甲。乙。丙。");
    assert_eq!(api.received_requests().await.unwrap().len(), 3);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_before_work_makes_no_provider_request() -> Result<(), Box<dyn Error>> {
    use beyond_slides::processing::StopSignal;
    let api = mock_api().await;
    let client = client(&api)?;
    let stop = StopSignal::default();
    stop.request_stop();
    let session = TranscriptRestorationSession::prepare(&client, transcript(), config(2)?)?
        .with_stop_signal(stop);
    assert!(matches!(
        session.complete_restoration().await,
        Err(RestorationSessionError::Stopped)
    ));
    assert!(api.received_requests().await.unwrap().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restoration_processes_all_missing_windows_immediately() -> Result<(), Box<dyn Error>> {
    let api = mock_api().await;
    let client = client(&api)?;
    let session = TranscriptRestorationSession::prepare(&client, transcript(), config(2)?)?;

    assert_eq!(session.window_count(), 3);
    assert_eq!(session.completed_window_count(), 0);

    let result = session.complete_restoration().await?;

    assert_eq!(result.transcript().text(), "甲。乙。丙。");
    assert_eq!(result.window_diagnostics().len(), 3);
    assert_eq!(
        api.received_requests()
            .await
            .expect("mock request recording is enabled")
            .len(),
        3
    );
    Ok(())
}

fn config(concurrency: usize) -> Result<TranscriptRestorationConfig, Box<dyn Error>> {
    Ok(TranscriptRestorationConfig::new(
        WindowingConfig::new(1, Duration::from_secs(60), 1)?,
        concurrency,
    )?)
}

fn transcript() -> Transcript {
    Transcript {
        segments: ["甲", "乙", "丙"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| TranscriptSegment {
                id: TranscriptSegmentId(index as u32),
                start_ms: Some(index as u64 * 1_000),
                end_ms: Some((index as u64 + 1) * 1_000),
                text: text.into(),
            })
            .collect(),
    }
}

fn client(api: &MockServer) -> Result<ChatCompletionsClient, Box<dyn Error>> {
    Ok(ChatCompletionsClient::new(ChatCompletionsConfig::new(
        format!("{}/v1", api.uri()),
        "test-key",
        "test-model",
    )?))
}

fn restoration_response(segment: u32, text: &str) -> Value {
    json!({
        "id": format!("chat-{segment}"),
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "content": json!({
                    "spans": [{
                        "kind": "text",
                        "source_start": segment,
                        "source_end": segment,
                        "text": text
                    }]
                }).to_string(),
                "tool_calls": []
            }
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 10
        }
    })
}

async fn mock_api() -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(RestorationResponder)
        .mount(&server)
        .await;
    server
}

struct RestorationResponder;

impl Respond for RestorationResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("request JSON");
        let input: Value = serde_json::from_str(
            body["messages"][1]["content"]
                .as_str()
                .expect("the second message contains the restoration task"),
        )
        .expect("restoration task JSON");
        let segment = input["owned_region"][0]["id"]
            .as_u64()
            .expect("owned segment ID") as u32;
        let text = match segment {
            0 => "甲。",
            1 => "乙。",
            2 => "丙。",
            _ => panic!("unexpected segment {segment}"),
        };
        ResponseTemplate::new(200).set_body_json(restoration_response(segment, text))
    }
}
