use std::{collections::VecDeque, error::Error, sync::Mutex, time::Duration};

use beyond_slides::{
    AnalysisAssemblyError, ChatCompletionsConfig, ChatCompletionsError, ModelExchangeTrace,
    ModelRequestKind, ModelTraceEvent, RestoredTranscript, RestoredTranscriptSpan, SearchError,
    Slide, SlideDeck, SlideId, SlideScore, SlideScorer, Transcript, TranscriptSegment,
    TranscriptSegmentId, TranscriptWindowTask, ValidatedSources, ValidationError, WindowingConfig,
    build_annotation_tasks, build_restoration_tasks, build_restored_annotation_tasks,
    build_restored_windows, build_windows, read_model_trace,
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
    assert_eq!(result.analysis.passages[0].start, TranscriptSegmentId(0));
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
async fn restored_annotation_accepts_small_copying_errors_without_a_model_repair()
-> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let restored = restored_transcript();
    let task = restored_task(&sources, &restored)?;
    let proposed_text = "二分查找通过不断缩小搜索区间来定位目标，并用循环不变式解释算法为何正确。";
    let api = mock_api(vec![final_response(
        "chat-1",
        restored_analysis_json(proposed_text),
    )])
    .await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client
        .annotate_restored_window(&sources, &scorer, &task)
        .await?;

    assert_eq!(result.analysis.passages[0].text, restored.text());
    assert_eq!(result.projection.changed_characters, 1);
    assert!(!result.projection.is_exact());
    assert_eq!(result.diagnostics.final_answer_repairs, 0);

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    let request: Value = requests[0].body_json()?;
    assert_eq!(request["tool_choice"], "auto");
    assert!(
        request["messages"][0]["content"]
            .as_str()
            .expect("system instructions are text")
            .contains("逐字符完全相同")
    );
    assert_eq!(
        request["messages"][1]["content"]
            .as_str()
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .expect("user message is JSON")["owned_text"],
        restored.text()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restored_annotation_repairs_text_beyond_the_five_percent_limit()
-> Result<(), Box<dyn Error>> {
    let sources = sources()?;
    let restored = restored_transcript();
    let task = restored_task(&sources, &restored)?;
    let invalid = restored_analysis_json("这段回答已经完全改写，不能作为原文分界依据。");
    let api = mock_api(vec![
        final_response("chat-1", invalid.clone()),
        final_response("chat-2", restored_analysis_json(&restored.text())),
    ])
    .await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client
        .annotate_restored_window(&sources, &scorer, &task)
        .await?;

    assert!(result.projection.is_exact());
    assert_eq!(result.diagnostics.final_answer_repairs, 1);
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    assert_eq!(requests.len(), 2);
    let repair_request: Value = requests[1].body_json()?;
    assert_eq!(repair_request["messages"][2]["content"], invalid);
    assert!(
        repair_request["messages"][3]["content"]
            .as_str()
            .expect("repair instruction is text")
            .contains("逐字符完全相同")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restoration_uses_json_mode_without_annotation_tools() -> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![final_response("chat-1", restoration_json())]).await;
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "test-model")?
        .with_max_output_tokens(2_048)?
        .with_extra_body(json!({ "thinking": { "type": "disabled" } }))?;
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    );
    let task = build_restoration_tasks(&windows)
        .into_iter()
        .next()
        .expect("the transcript creates one restoration task");

    let result = client.restore_window(&task).await?;

    assert_eq!(result.restoration.spans.len(), 1);
    assert_eq!(result.diagnostics.final_answer_repairs, 0);
    assert_eq!(result.diagnostics.prompt_tokens, Some(20));
    assert_eq!(result.diagnostics.completion_tokens, Some(10));

    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    let request: Value = requests[0].body_json()?;
    assert_eq!(request["response_format"], json!({ "type": "json_object" }));
    assert_eq!(request["thinking"], json!({ "type": "disabled" }));
    assert!(request.get("tools").is_none());
    assert!(request.get("tool_choice").is_none());
    assert!(
        request["messages"][0]["content"]
            .as_str()
            .expect("system instructions are text")
            .contains("TranscriptWindowRestoration")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_restoration_is_returned_to_the_model_for_repair() -> Result<(), Box<dyn Error>> {
    let invalid = json!({
        "spans": [{
            "kind": "text",
            "source_start": 1,
            "source_end": 1,
            "text": "错误的来源范围。"
        }]
    })
    .to_string();
    let api = mock_api(vec![
        final_response("chat-1", invalid.clone()),
        final_response("chat-2", restoration_json()),
    ])
    .await;
    let client = beyond_slides::ChatCompletionsClient::new(ChatCompletionsConfig::new(
        base_url(&api),
        "test-key",
        "test-model",
    )?);
    let sources = sources()?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    );
    let task = build_restoration_tasks(&windows)
        .into_iter()
        .next()
        .expect("the transcript creates one restoration task");

    let result = client.restore_window(&task).await?;

    assert_eq!(result.diagnostics.final_answer_repairs, 1);
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    let repair_request: Value = requests[1].body_json()?;
    assert_eq!(repair_request["messages"][2]["content"], invalid);
    assert!(
        repair_request["messages"][3]["content"]
            .as_str()
            .expect("repair instruction is text")
            .contains("owned_region")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn trace_records_requests_responses_and_validation_repairs_without_credentials()
-> Result<(), Box<dyn Error>> {
    let invalid = json!({
        "spans": [{
            "kind": "text",
            "source_start": 1,
            "source_end": 1,
            "text": "错误的来源范围。"
        }]
    })
    .to_string();
    let api = mock_api(vec![
        final_response("chat-1", invalid),
        final_response("chat-2", restoration_json()),
    ])
    .await;
    let directory = tempfile::tempdir()?;
    let trace_path = directory.path().join("model-trace.jsonl");
    let trace = ModelExchangeTrace::open(&trace_path)?;
    let config = ChatCompletionsConfig::new(base_url(&api), "secret-test-key", "test-model")?
        .with_extra_body(json!({ "thinking": { "type": "disabled" } }))?
        .with_model_trace(trace);
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    );
    let task = build_restoration_tasks(&windows)
        .into_iter()
        .next()
        .expect("the transcript creates one restoration task");

    client.restore_window(&task).await?;

    let trace_text = std::fs::read_to_string(&trace_path)?;
    assert!(!trace_text.contains("secret-test-key"));
    assert!(!trace_text.to_ascii_lowercase().contains("authorization"));
    let records = read_model_trace(&trace_path)?;
    assert_eq!(records.len(), 6);
    assert!(matches!(
        &records[0].event,
        ModelTraceEvent::Request { options, .. }
            if options["extra_body"] == json!({ "thinking": { "type": "disabled" } })
    ));
    assert!(matches!(
        &records[1].event,
        ModelTraceEvent::Response { raw_response: Some(body), .. }
            if body["id"] == "chat-1"
    ));
    assert!(matches!(
        &records[2].event,
        ModelTraceEvent::Validation {
            accepted: false,
            category: Some(category),
            ..
        } if category == "outside_owned_region"
    ));
    assert_eq!(records[3].request_kind, ModelRequestKind::Repair);
    assert!(matches!(
        &records[5].event,
        ModelTraceEvent::Validation {
            accepted: true,
            category: None,
            error: None,
        }
    ));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn trace_records_each_provider_retry_attempt() -> Result<(), Box<dyn Error>> {
    let api = MockServer::builder().start().await;
    Mock::given(any())
        .respond_with(FailOnceThenRespond {
            failed: Mutex::new(false),
            response: final_response("chat-2", restoration_json()),
        })
        .mount(&api)
        .await;
    let directory = tempfile::tempdir()?;
    let trace_path = directory.path().join("model-trace.jsonl");
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "test-model")?
        .with_max_provider_retries(1)
        .with_model_trace(ModelExchangeTrace::open(&trace_path)?);
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let windows = build_windows(
        &sources,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    );
    let task = build_restoration_tasks(&windows)
        .into_iter()
        .next()
        .expect("the transcript creates one restoration task");

    let result = client.restore_window(&task).await?;

    assert_eq!(result.diagnostics.provider_retries, 1);
    let records = read_model_trace(&trace_path)?;
    assert_eq!(records.len(), 5);
    assert!(matches!(
        &records[1].event,
        ModelTraceEvent::ProviderError {
            provider_attempt: 0,
            retryable: true,
            will_retry: true,
            error,
            ..
        } if error.status == Some(503)
    ));
    assert!(matches!(
        &records[2].event,
        ModelTraceEvent::Request {
            provider_attempt: 1,
            ..
        }
    ));
    assert_ne!(records[0].exchange_id, records[2].exchange_id);
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
    let final_request: Value = requests[1].body_json()?;
    assert!(final_request.get("tools").is_none());
    assert!(final_request.get("tool_choice").is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn last_tool_round_forces_a_tool_free_final_response() -> Result<(), Box<dyn Error>> {
    let api = mock_api(vec![
        tool_response(
            "chat-1",
            vec![tool_call("call-1", "inspect_slide", r#"{"slide_id":4}"#)],
        ),
        final_response("chat-2", analysis_json()),
    ])
    .await;
    let config = ChatCompletionsConfig::new(base_url(&api), "test-key", "test-model")?
        .with_max_tool_rounds(1)?;
    let client = beyond_slides::ChatCompletionsClient::new(config);
    let sources = sources()?;
    let task = task(&sources)?;
    let scorer = FixedScorer(scores([0.0; 6]));

    let result = client.annotate_window(&sources, &scorer, &task).await?;

    assert_eq!(result.diagnostics.tool_rounds, 1);
    let requests = api
        .received_requests()
        .await
        .expect("mock request recording is enabled");
    let final_request: Value = requests[1].body_json()?;
    assert!(final_request.get("tools").is_none());
    assert!(final_request.get("tool_choice").is_none());
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
                passage_start: TranscriptSegmentId(0),
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

fn restoration_json() -> String {
    json!({
        "spans": [{
            "kind": "text",
            "source_start": 0,
            "source_end": 0,
            "text": "二分查找的课堂讲解。"
        }]
    })
    .to_string()
}

fn restored_analysis_json(text: &str) -> String {
    json!({
        "passages": [{
            "text": text,
            "novelty": 2,
            "connection_strength": 3,
            "importance": 4,
            "related_slides": [0]
        }]
    })
    .to_string()
}

fn restored_transcript() -> RestoredTranscript {
    RestoredTranscript {
        spans: vec![RestoredTranscriptSpan::Text {
            source_start: TranscriptSegmentId(0),
            source_end: TranscriptSegmentId(0),
            text: "二分查找通过不断缩小搜索区间来定位目标，并使用循环不变式解释算法为何正确。"
                .into(),
        }],
    }
}

fn sources() -> Result<ValidatedSources, Box<dyn Error>> {
    Ok(ValidatedSources::new(
        Transcript {
            segments: vec![TranscriptSegment {
                id: TranscriptSegmentId(0),
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

fn restored_task<'a>(
    sources: &'a ValidatedSources,
    restored: &'a RestoredTranscript,
) -> Result<beyond_slides::RestoredTranscriptWindowTask<'a>, Box<dyn Error>> {
    let windows = build_restored_windows(
        sources,
        restored,
        WindowingConfig::new(100, Duration::from_secs(60), 20)?,
    )?;
    Ok(
        build_restored_annotation_tasks(sources, &windows, &[SlideId(0)])?
            .into_iter()
            .next()
            .expect("the restored transcript creates one annotation task"),
    )
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

struct FailOnceThenRespond {
    failed: Mutex<bool>,
    response: Value,
}

impl Respond for FailOnceThenRespond {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let mut failed = self.failed.lock().expect("lock mock provider state");
        if !*failed {
            *failed = true;
            ResponseTemplate::new(503).set_body_json(json!({ "error": "temporarily unavailable" }))
        } else {
            ResponseTemplate::new(200).set_body_json(self.response.clone())
        }
    }
}
