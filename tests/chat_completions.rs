use std::{collections::VecDeque, error::Error, sync::Mutex, time::Duration};

use beyond_slides::{
    AnalysisAssemblyError, ChatCompletionsConfig, ChatCompletionsError, SearchError, SentenceId,
    Slide, SlideDeck, SlideId, SlideScore, SlideScorer, Transcript, TranscriptSentence,
    TranscriptWindowTask, ValidatedSources, ValidationError, WindowingConfig,
    build_annotation_tasks, build_windows,
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
async fn annotation_uses_the_configured_openai_compatible_endpoint() -> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![final_response("chat-1", analysis_json())]).await;
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "GLM-5")?
        .with_max_output_tokens(2_048)?;
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client.annotate_window(&sources, &scorer, &task).await?;

    assert_eq!(result.analysis.passages.len(), 1);
    assert_eq!(result.analysis.passages[0].start, SentenceId(0));
    assert_eq!(result.diagnostics.prompt_tokens, Some(20));
    assert_eq!(result.diagnostics.completion_tokens, Some(10));
    assert_eq!(result.diagnostics.tool_rounds, 0);
    assert!(!result.diagnostics.accepted_json_fence);

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].url.path(), "/v1/chat/completions");
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer test-key")
    );

    let request: Value = requests[0].body_json()?;
    assert_eq!(request["model"], "GLM-5");
    assert_eq!(request["tool_choice"], "auto");
    assert_eq!(request["response_format"], json!({ "type": "json_object" }));
    assert_eq!(request["max_tokens"], 2_048);
    assert_eq!(request["stream"], false);
    let tool_names: Vec<_> = request["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .map(|tool| tool["function"]["name"].as_str().expect("tool name"))
        .collect();
    assert_eq!(tool_names, ["inspect_slide", "search_slides"]);
    assert_eq!(request["messages"][0]["role"], "system");
    assert_eq!(request["messages"][1]["role"], "user");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_results_and_correctable_errors_are_replayed_by_call_id() -> Result<(), Box<dyn Error>>
{
    let api = mock_api(vec![
        tool_response(
            "chat-1",
            vec![
                tool_call("call-unknown", "invented_tool", r#"{}"#),
                tool_call("call-inspect", "inspect_slide", r#"{"slide_id":4}"#),
            ],
        ),
        tool_response(
            "chat-2",
            vec![tool_call(
                "call-malformed",
                "inspect_slide",
                r#"{"slide_id":"four"}"#,
            )],
        ),
        final_response("chat-3", format!("```json\n{}\n```", analysis_json())),
    ])
    .await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client.annotate_window(&sources, &scorer, &task).await?;

    assert_eq!(result.diagnostics.tool_rounds, 2);
    assert!(result.diagnostics.accepted_json_fence);
    assert_eq!(result.diagnostics.prompt_tokens, Some(60));
    assert_eq!(result.diagnostics.completion_tokens, Some(30));

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    let after_first_round: Value = requests[1].body_json()?;
    let first_results = tool_messages(&after_first_round);
    assert_eq!(first_results.len(), 2);
    assert_eq!(first_results[0]["tool_call_id"], "call-unknown");
    assert!(
        first_results[0]["content"]
            .as_str()
            .expect("tool content is a JSON string")
            .contains(r#""code":"unknown_tool""#)
    );
    assert_eq!(first_results[1]["tool_call_id"], "call-inspect");
    assert!(
        first_results[1]["content"]
            .as_str()
            .expect("tool content is a JSON string")
            .contains("第五张：终止条件")
    );

    let after_second_round: Value = requests[2].body_json()?;
    let all_results = tool_messages(&after_second_round);
    assert_eq!(all_results.len(), 3);
    assert_eq!(all_results[2]["tool_call_id"], "call-malformed");
    assert!(
        all_results[2]["content"]
            .as_str()
            .expect("tool content is a JSON string")
            .contains(r#""code":"invalid_arguments""#)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_round_limit_stops_an_unproductive_conversation() -> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![
        tool_response(
            "chat-1",
            vec![tool_call("call-1", "invented_tool", r#"{}"#)],
        ),
        tool_response(
            "chat-2",
            vec![tool_call("call-2", "invented_tool", r#"{}"#)],
        ),
    ])
    .await;
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "test-model")?
        .with_max_tool_rounds(1)?;
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let error = client
        .annotate_window(&sources, &scorer, &task)
        .await
        .expect_err("a second tool round exceeds the configured limit");

    assert!(matches!(
        error,
        ChatCompletionsError::ToolRoundLimit { limit: 1 }
    ));
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 2);
    let first_request: Value = requests[0].body_json()?;
    assert!(
        first_request["messages"][0]["content"]
            .as_str()
            .expect("system instructions are text")
            .contains("最多可以进行 1 轮工具调用")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn construction_performs_no_model_discovery() -> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![]).await;
    let _client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);

    assert_eq!(
        api.received_requests()
            .await
            .expect("mock request recording is enabled")
            .len(),
        0
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn incomplete_or_invalid_usage_makes_token_totals_unknown() -> Result<(), Box<dyn Error>> {
    let mut response = final_response("chat-1", analysis_json());
    response["usage"] = json!({ "prompt_tokens": -1 });
    let api = mock_api(vec![response]).await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client.annotate_window(&sources, &scorer, &task).await?;

    assert_eq!(result.diagnostics.prompt_tokens, None);
    assert_eq!(result.diagnostics.completion_tokens, None);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_final_json_is_returned_to_the_model_for_repair() -> Result<(), Box<dyn Error>> {
    let incomplete_analysis = json!({
        "passages": [{
            "start": 0,
            "end": 0,
            "novelty": 1,
            "connection_strength": 2,
            "importance": 3
        }]
    })
    .to_string();
    let api = mock_api(vec![
        final_response("chat-1", incomplete_analysis.clone()),
        final_response("chat-2", analysis_json()),
    ])
    .await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client.annotate_window(&sources, &scorer, &task).await?;

    assert_eq!(result.diagnostics.final_answer_repairs, 1);
    assert_eq!(result.diagnostics.prompt_tokens, Some(40));
    assert_eq!(result.diagnostics.completion_tokens, Some(20));
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 2);
    let repair_request: Value = requests[1].body_json()?;
    assert_eq!(repair_request["messages"][2]["role"], "assistant");
    assert_eq!(
        repair_request["messages"][2]["content"],
        incomplete_analysis
    );
    assert_eq!(repair_request["messages"][3]["role"], "user");
    let repair_instruction = repair_request["messages"][3]["content"]
        .as_str()
        .expect("repair instruction is text");
    assert!(repair_instruction.contains("missing field `related_slides`"));
    assert!(repair_instruction.contains("完整"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_window_analysis_fails_after_the_repair_limit() -> Result<(), Box<dyn Error>> {
    let invalid_analysis = json!({
        "passages": [{
            "start": 0,
            "end": 0,
            "novelty": 1,
            "connection_strength": 2,
            "importance": 3,
            "related_slides": [99]
        }]
    })
    .to_string();
    let api = mock_api(vec![
        final_response("chat-1", invalid_analysis.clone()),
        final_response("chat-2", invalid_analysis),
    ])
    .await;
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "test-model")?
        .with_max_final_answer_repairs(1)?;
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let error = client
        .annotate_window(&sources, &scorer, &task)
        .await
        .expect_err("an unknown related slide must fail after one repair");

    let ChatCompletionsError::FinalAnswerRepairLimit { limit, source } = error else {
        panic!("expected repair-limit error");
    };
    assert_eq!(limit, 1);
    assert!(matches!(
        *source,
        ChatCompletionsError::InvalidWindowAnalysis(AnalysisAssemblyError::InvalidAnalysis(
            ValidationError::UnknownRelatedSlide {
                passage_start: SentenceId(0),
                slide: SlideId(99),
            }
        ))
    ));
    assert_eq!(
        api.received_requests()
            .await
            .expect("mock request recording is enabled")
            .len(),
        2
    );
    Ok(())
}

fn tool_messages(request: &Value) -> Vec<&Value> {
    request["messages"]
        .as_array()
        .expect("messages is an array")
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect()
}

fn final_response(id: &str, content: impl Into<String>) -> Value {
    json!({
        "id": id,
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "content": content.into(),
                "tool_calls": []
            }
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 10
        }
    })
}

fn tool_response(id: &str, tool_calls: Vec<Value>) -> Value {
    json!({
        "id": id,
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "content": "",
                "tool_calls": tool_calls
            }
        }],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 10
        }
    })
}

fn tool_call(id: &str, name: &str, arguments: &str) -> Value {
    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments
        }
    })
}

fn analysis_json() -> String {
    json!({
        "passages": [{
            "start": 0,
            "end": 0,
            "novelty": 1,
            "connection_strength": 2,
            "importance": 3,
            "related_slides": [0]
        }]
    })
    .to_string()
}

fn sources() -> Result<ValidatedSources, Box<dyn Error>> {
    Ok(ValidatedSources::new(
        Transcript {
            sentences: vec![TranscriptSentence {
                id: SentenceId(0),
                start_ms: 0,
                end_ms: 1_000,
                text: "二分查找的课堂讲解".into(),
            }],
        },
        SlideDeck {
            slides: vec![
                slide(0, "第一张：二分查找"),
                slide(1, "第二张：循环不变式"),
                slide(2, "第三张：边界条件"),
                slide(3, "第四张：复杂度"),
                slide(4, "第五张：终止条件"),
                slide(5, "第六张：练习"),
            ],
        },
    )?)
}

fn task(sources: &ValidatedSources) -> Result<TranscriptWindowTask<'_>, Box<dyn Error>> {
    let windows = build_windows(
        sources,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    );
    Ok(build_annotation_tasks(sources, &windows, &[SlideId(0)])?
        .into_iter()
        .next()
        .expect("the transcript creates one annotation task"))
}

fn slide(id: u32, text: &str) -> Slide {
    Slide {
        id: SlideId(id),
        text: text.into(),
    }
}

fn scores(values: [f64; 6]) -> Vec<SlideScore> {
    values
        .into_iter()
        .enumerate()
        .map(|(slide_id, score)| SlideScore {
            slide_id: SlideId(slide_id as u32),
            score,
        })
        .collect()
}

async fn mock_api(responses: Vec<Value>) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(ResponseSequence(Mutex::new(responses.into())))
        .mount(&server)
        .await;
    server
}

fn base_url(server: &MockServer) -> String {
    format!("{}/v1", server.uri())
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
