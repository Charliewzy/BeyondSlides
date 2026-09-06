use std::{collections::VecDeque, error::Error, sync::Mutex};

use beyond_slides::{
    ChatCompletionsClient, ChatCompletionsConfig, ComparativeMetric, ComparativeRankingConfig,
    ComparativeRankingSession, RestoredLecturePassage, RestoredTranscript, RestoredTranscriptSpan,
    Score5, SearchError, Slide, SlideDeck, SlideId, SlideScore, SlideScorer, Transcript,
    TranscriptSegment, TranscriptSegmentId, ValidatedRestoredAnalysis, ValidatedSources,
};
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

struct FixedScorer(Vec<SlideScore>);

impl SlideScorer for FixedScorer {
    fn score_slides(&self, _query: &str) -> Result<Vec<SlideScore>, SearchError> {
        Ok(self.0.clone())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_repairs_metric_canaries_and_keeps_their_inputs_separate()
-> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![
        final_response(json!({
            "comparisons": [
                {"comparison_id": 0, "most": "E", "least": "D"},
                {"comparison_id": 1, "most": "A", "least": "C"}
            ]
        })),
        final_response(json!({
            "comparisons": [
                {"comparison_id": 0, "most": "A", "least": "D"},
                {"comparison_id": 1, "most": "B", "least": "C"}
            ]
        })),
        final_response(json!({
            "comparisons": [
                {"comparison_id": 0, "most": "C", "least": "B"},
                {"comparison_id": 1, "most": "D", "least": "A"}
            ]
        })),
    ])
    .await;
    let client = ChatCompletionsClient::new(ChatCompletionsConfig::new(
        format!("{}/v1", api.uri()),
        "test-key",
        "test-model",
    )?);
    let analysis = analysis()?;
    let scorer = FixedScorer(vec![
        SlideScore {
            slide_id: SlideId(0),
            score: 0.8,
        },
        SlideScore {
            slide_id: SlideId(1),
            score: 0.4,
        },
    ]);
    let config = ComparativeRankingConfig::new(2)?.with_rounds(2)?;
    let session = ComparativeRankingSession::prepare(&client, &analysis, &scorer, config)?;
    assert_eq!(session.batch_count(), 2);

    let mut completed_metrics = Vec::new();
    let result = session
        .complete_with_progress(|event| {
            completed_metrics.push(event.batch.metric);
            Ok(())
        })
        .await?;

    assert_eq!(
        completed_metrics,
        vec![ComparativeMetric::Importance, ComparativeMetric::Novelty]
    );
    assert_eq!(result.batch_results().len(), 2);
    assert_eq!(
        result.batch_results()[0].diagnostics.final_answer_repairs,
        1
    );
    assert_eq!(result.rankings().importance.len(), 4);
    assert_eq!(result.rankings().novelty.len(), 4);

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 3);
    let importance = user_input(&requests[0])?;
    assert!(importance.get("slides").is_none());
    assert!(importance.get("passages").is_none());
    let candidate = &importance["comparisons"][0]["candidates"]["A"];
    assert!(candidate["text"].is_string());
    assert!(candidate.get("passage_id").is_none());
    assert!(candidate.get("slide_position").is_none());
    assert!(candidate.get("novelty").is_none());
    assert!(candidate.get("importance").is_none());

    let novelty = user_input(&requests[2])?;
    assert!(
        !novelty["slides"]
            .as_array()
            .expect("slides array")
            .is_empty()
    );
    let candidate = &novelty["comparisons"][0]["candidates"]["A"];
    assert!(candidate.get("slide_position").is_some());
    for id in candidate["candidate_slide_ids"].as_array().unwrap() {
        assert!(
            novelty["slides"]
                .as_array()
                .unwrap()
                .iter()
                .any(|slide| slide["slide_id"] == *id)
        );
    }
    for request in requests {
        let body: Value = request.body_json()?;
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }
    Ok(())
}

fn analysis() -> Result<ValidatedRestoredAnalysis, Box<dyn Error>> {
    let raw_texts = ["甲", "乙", "丙", "丁"];
    let restored_texts = ["甲。", "乙。", "丙。", "丁。"];
    let transcript = Transcript {
        segments: raw_texts
            .iter()
            .enumerate()
            .map(|(index, text)| TranscriptSegment {
                id: TranscriptSegmentId(index as u32),
                start_ms: index as u64 * 1_000,
                end_ms: (index as u64 + 1) * 1_000,
                text: (*text).into(),
            })
            .collect(),
    };
    let slides = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(0),
                text: "甲与乙".into(),
            },
            Slide {
                id: SlideId(1),
                text: "丙与丁".into(),
            },
        ],
    };
    let sources = ValidatedSources::new(transcript, slides)?;
    let restored_transcript = RestoredTranscript {
        spans: restored_texts
            .iter()
            .enumerate()
            .map(|(index, text)| RestoredTranscriptSpan::Text {
                source_start: TranscriptSegmentId(index as u32),
                source_end: TranscriptSegmentId(index as u32),
                text: (*text).into(),
            })
            .collect(),
    };
    let passages = restored_texts
        .iter()
        .enumerate()
        .map(|(index, text)| RestoredLecturePassage {
            text: (*text).into(),
            source_start: TranscriptSegmentId(index as u32),
            source_end: TranscriptSegmentId(index as u32),
            slide_position: SlideId((index / 2) as u32),
            novelty: Score5::ZERO,
            connection_strength: Some(Score5::try_from(2).expect("valid score")),
            importance: Score5::ZERO,
            comparative_novelty: None,
            comparative_importance: None,
            related_slides: vec![SlideId((index / 2) as u32)],
            summary: None,
            comparison_note: None,
        })
        .collect();
    Ok(ValidatedRestoredAnalysis::new(
        sources,
        restored_transcript,
        passages,
    )?)
}

fn final_response(content: Value) -> Value {
    json!({
        "id": "comparison",
        "choices": [{
            "finish_reason": "stop",
            "message": {"content": content.to_string(), "tool_calls": []}
        }],
        "usage": {"prompt_tokens": 20, "completion_tokens": 10}
    })
}

fn user_input(request: &Request) -> Result<Value, Box<dyn Error>> {
    let body: Value = request.body_json()?;
    Ok(serde_json::from_str(
        body["messages"][1]["content"]
            .as_str()
            .expect("user message is text"),
    )?)
}

async fn mock_api(responses: Vec<Value>) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(ResponseSequence(Mutex::new(responses.into())))
        .mount(&server)
        .await;
    server
}

struct ResponseSequence(Mutex<VecDeque<Value>>);

impl Respond for ResponseSequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let response = self
            .0
            .lock()
            .expect("lock mock response sequence")
            .pop_front()
            .expect("a response exists for every expected request");
        ResponseTemplate::new(200).set_body_json(response)
    }
}
