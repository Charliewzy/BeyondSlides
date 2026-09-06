use std::{
    error::Error,
    fmt, io,
    sync::Mutex,
    time::{Duration, Instant},
};

use genai::{
    Client, ModelIden, ServiceTarget,
    adapter::AdapterKind,
    chat::{
        ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatResponseFormat, StopReason, Tool,
        ToolCall, ToolChoice, ToolResponse,
    },
    resolver::{AuthData, Endpoint},
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::{
    ComparativeMetric, ComparativeRankingBatchResult, ComparativeRankingValidationError,
    ModelExchangeTrace, ModelProviderError, ModelRequestKind, ModelWorkflow,
    PassageProjectionDiagnostics, PassageProjectionError, SlideId, SlideScorer, ValidatedSources,
    annotation::{
        AnalysisAssemblyError, AnnotationToolError, AnnotationToolSession,
        TranscriptWindowAnalysis, TranscriptWindowTask, validate_window_analysis,
    },
    comparative_ranking::{ComparativeRankingTask, ProposedComparativeRanking},
    model_trace::{ModelProviderFailure, ModelTraceContext},
    restoration::{
        RestorationError, TranscriptRestorationTask, TranscriptWindowRestoration,
        validate_window_restoration,
    },
    restored_annotation::{
        ProposedTranscriptWindowAnalysis, RestoredAnnotationError,
        RestoredTranscriptWindowAnalysis, RestoredTranscriptWindowTask, project_window_analysis,
    },
};

const DEFAULT_MAX_TOOL_ROUNDS: usize = 4;
const DEFAULT_MAX_FINAL_ANSWER_REPAIRS: usize = 2;
const DEFAULT_MAX_SEARCH_RESULTS: usize = 5;
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;
const MAX_PROVIDER_ERROR_CHARACTERS: usize = 2_000;

/// Configuration for one OpenAI-compatible Chat Completions endpoint.
///
/// BeyondSlides always selects the OpenAI-compatible protocol explicitly. The
/// model name is sent to the configured endpoint without using genai's
/// model-to-provider inference.
pub struct ChatCompletionsConfig {
    base_url: Url,
    api_key: String,
    model: String,
    max_tool_rounds: usize,
    max_final_answer_repairs: usize,
    max_search_results: usize,
    max_output_tokens: u32,
    max_provider_retries: usize,
    minimum_request_interval: Duration,
    extra_body: Option<Value>,
    model_trace: Option<ModelExchangeTrace>,
}

impl ChatCompletionsConfig {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ChatCompletionsConfigError> {
        let mut base_url = base_url.into();
        if !base_url.ends_with('/') {
            base_url.push('/');
        }
        let base_url = Url::parse(&base_url)
            .map_err(|error| ChatCompletionsConfigError::InvalidBaseUrl(error.to_string()))?;
        if base_url.cannot_be_a_base()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(ChatCompletionsConfigError::InvalidBaseUrl(
                "the base URL must be hierarchical and contain no query or fragment".into(),
            ));
        }

        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(ChatCompletionsConfigError::EmptyApiKey);
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(ChatCompletionsConfigError::EmptyModel);
        }

        Ok(Self {
            base_url,
            api_key,
            model,
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            max_final_answer_repairs: DEFAULT_MAX_FINAL_ANSWER_REPAIRS,
            max_search_results: DEFAULT_MAX_SEARCH_RESULTS,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            max_provider_retries: 0,
            minimum_request_interval: Duration::ZERO,
            extra_body: None,
            model_trace: None,
        })
    }

    pub fn with_max_final_answer_repairs(
        mut self,
        max_final_answer_repairs: usize,
    ) -> Result<Self, ChatCompletionsConfigError> {
        if max_final_answer_repairs == 0 {
            return Err(ChatCompletionsConfigError::ZeroLimit(
                "max_final_answer_repairs",
            ));
        }
        self.max_final_answer_repairs = max_final_answer_repairs;
        Ok(self)
    }

    pub fn with_max_tool_rounds(
        mut self,
        max_tool_rounds: usize,
    ) -> Result<Self, ChatCompletionsConfigError> {
        if max_tool_rounds == 0 {
            return Err(ChatCompletionsConfigError::ZeroLimit("max_tool_rounds"));
        }
        self.max_tool_rounds = max_tool_rounds;
        Ok(self)
    }

    pub fn with_max_search_results(
        mut self,
        max_search_results: usize,
    ) -> Result<Self, ChatCompletionsConfigError> {
        if max_search_results == 0 {
            return Err(ChatCompletionsConfigError::ZeroLimit("max_search_results"));
        }
        self.max_search_results = max_search_results;
        Ok(self)
    }

    pub fn with_max_output_tokens(
        mut self,
        max_output_tokens: u32,
    ) -> Result<Self, ChatCompletionsConfigError> {
        if max_output_tokens == 0 {
            return Err(ChatCompletionsConfigError::ZeroLimit("max_output_tokens"));
        }
        self.max_output_tokens = max_output_tokens;
        Ok(self)
    }

    pub const fn with_max_provider_retries(mut self, max_provider_retries: usize) -> Self {
        self.max_provider_retries = max_provider_retries;
        self
    }

    /// Sets the minimum time between HTTP request starts across this client.
    pub const fn with_minimum_request_interval(mut self, interval: Duration) -> Self {
        self.minimum_request_interval = interval;
        self
    }

    /// Records complete model exchanges without exposing credentials to the trace.
    pub fn with_model_trace(mut self, model_trace: ModelExchangeTrace) -> Self {
        self.model_trace = Some(model_trace);
        self
    }

    /// Adds provider-specific top-level request fields after validating that
    /// they can be merged into the Chat Completions request object.
    pub fn with_extra_body(
        mut self,
        extra_body: Value,
    ) -> Result<Self, ChatCompletionsConfigError> {
        if !extra_body.is_object() {
            return Err(ChatCompletionsConfigError::ExtraBodyNotObject);
        }
        self.extra_body = Some(extra_body);
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCompletionsConfigError {
    InvalidBaseUrl(String),
    EmptyApiKey,
    EmptyModel,
    ExtraBodyNotObject,
    ZeroLimit(&'static str),
}

impl fmt::Display for ChatCompletionsConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl(error) => {
                write!(formatter, "invalid Chat Completions base URL: {error}")
            }
            Self::EmptyApiKey => {
                formatter.write_str("the Chat Completions API key cannot be empty")
            }
            Self::EmptyModel => {
                formatter.write_str("the Chat Completions model name cannot be empty")
            }
            Self::ExtraBodyNotObject => {
                formatter.write_str("the Chat Completions extra body must be a JSON object")
            }
            Self::ZeroLimit(name) => {
                write!(
                    formatter,
                    "Chat Completions {name} must be greater than zero"
                )
            }
        }
    }
}

impl Error for ChatCompletionsConfigError {}

/// A client for one explicitly configured OpenAI-compatible endpoint.
///
/// Construction performs no network discovery. The first real transcript
/// window is the canary for endpoint access, model availability, tool calling,
/// and JSON output.
pub struct ChatCompletionsClient {
    transport: Client,
    target: ServiceTarget,
    options: ChatOptions,
    max_tool_rounds: usize,
    max_final_answer_repairs: usize,
    max_search_results: usize,
    max_provider_retries: usize,
    minimum_request_interval: Duration,
    next_request_at: Mutex<Option<Instant>>,
    endpoint: String,
    model: String,
    model_trace: Option<ModelExchangeTrace>,
}

impl ChatCompletionsClient {
    pub fn new(config: ChatCompletionsConfig) -> Self {
        let endpoint = config.base_url.to_string();
        let model = config.model.clone();
        let target = ServiceTarget {
            endpoint: Endpoint::from_owned(endpoint.clone()),
            auth: AuthData::from_single(config.api_key),
            model: ModelIden::new(AdapterKind::OpenAI, model.clone()),
        };
        let mut options = ChatOptions::default()
            .with_temperature(0.0)
            .with_max_tokens(config.max_output_tokens)
            .with_response_format(ChatResponseFormat::JsonMode)
            .with_capture_raw_body(true);
        if let Some(extra_body) = config.extra_body {
            options = options.with_extra_body(extra_body);
        }

        Self {
            transport: Client::default(),
            target,
            options,
            max_tool_rounds: config.max_tool_rounds,
            max_final_answer_repairs: config.max_final_answer_repairs,
            max_search_results: config.max_search_results,
            max_provider_retries: config.max_provider_retries,
            minimum_request_interval: config.minimum_request_interval,
            next_request_at: Mutex::new(None),
            endpoint,
            model,
            model_trace: config.model_trace,
        }
    }

    /// Runs the complete tool-calling conversation for one transcript window.
    pub async fn annotate_window(
        &self,
        sources: &ValidatedSources,
        scorer: &dyn SlideScorer,
        task: &TranscriptWindowTask<'_>,
    ) -> Result<AnnotationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let request = self.annotation_request(message.instructions, message.input);
        let outcome = self
            .run_conversation(
                request,
                AnnotationWorkflow::new(sources, scorer, task, self.max_search_results),
            )
            .await?;

        Ok(AnnotationResult {
            analysis: outcome.output,
            diagnostics: AnnotationDiagnostics::from(outcome.diagnostics),
        })
    }

    /// Annotates readable restored text and projects model boundaries onto it.
    pub async fn annotate_restored_window(
        &self,
        sources: &ValidatedSources,
        scorer: &dyn SlideScorer,
        task: &RestoredTranscriptWindowTask<'_>,
    ) -> Result<RestoredAnnotationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let request = self.annotation_request(message.instructions, message.input);
        let outcome = self
            .run_conversation(
                request,
                RestoredAnnotationWorkflow::new(sources, scorer, task, self.max_search_results),
            )
            .await?;

        Ok(RestoredAnnotationResult {
            analysis: outcome.output.analysis,
            projection: outcome.output.projection,
            diagnostics: AnnotationDiagnostics::from(outcome.diagnostics),
        })
    }

    fn annotation_request(&self, instructions: &str, input: String) -> ChatRequest {
        let instructions = format!(
            "{instructions}\n\n工具调用预算：最多可以进行 {} 轮工具调用。一轮可以同时调用多个工具。收到最后一轮的工具结果后，必须直接返回最终 JSON，不得继续调用工具。",
            self.max_tool_rounds
        );
        ChatRequest::from_user(input)
            .with_system(instructions)
            .with_tools(tool_definitions(self.max_search_results))
    }

    /// Restores one transcript window without exposing annotation tools.
    pub async fn restore_window(
        &self,
        task: &TranscriptRestorationTask<'_>,
    ) -> Result<TranscriptWindowRestorationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let request = ChatRequest::from_user(message.input).with_system(message.instructions);
        let outcome = self
            .run_conversation(request, RestorationWorkflow { task: *task })
            .await?;

        Ok(TranscriptWindowRestorationResult {
            restoration: outcome.output,
            diagnostics: RestorationDiagnostics::from(outcome.diagnostics),
        })
    }

    /// Runs one validated batch of lecture-wide best--worst comparisons.
    pub(crate) async fn compare_passages(
        &self,
        task: &ComparativeRankingTask,
    ) -> Result<ComparativeRankingBatchResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let request = ChatRequest::from_user(message.input).with_system(message.instructions);
        let outcome = self
            .run_conversation(request, ComparativeRankingWorkflow { task })
            .await?;

        Ok(ComparativeRankingBatchResult {
            metric: task.metric(),
            batch_index: task.batch_index(),
            comparisons: outcome.output,
            diagnostics: AnnotationDiagnostics::from(outcome.diagnostics),
        })
    }

    async fn run_conversation<W>(
        &self,
        mut request: ChatRequest,
        mut workflow: W,
    ) -> Result<ConversationOutcome<W::Output>, ChatCompletionsError>
    where
        W: ConversationWorkflow,
    {
        let mut diagnostics = ConversationDiagnostics::default();
        let mut conversation_turn = 0;
        let mut request_kind = ModelRequestKind::Initial;

        loop {
            let trace_context = workflow.trace_context(conversation_turn, request_kind);
            let tools_enabled = request.tools.is_some();
            let (response, provider_retries, exchange_id) = self
                .chat(request.clone(), tools_enabled, trace_context)
                .await?;
            if let Err(error) = ensure_one_choice(&response) {
                return self.processing_failure(exchange_id, trace_context, "choice_count", error);
            }
            diagnostics.record(&response, provider_retries);
            let tool_calls: Vec<_> = response.tool_calls().into_iter().cloned().collect();

            if !tool_calls.is_empty() {
                if !matches!(response.stop_reason.as_ref(), Some(StopReason::ToolCall(_))) {
                    let error = unexpected_finish_reason(&response, true);
                    return self.processing_failure(
                        exchange_id,
                        trace_context,
                        "unexpected_finish_reason",
                        error,
                    );
                }
                if diagnostics.tool_rounds == self.max_tool_rounds {
                    let error = ChatCompletionsError::ToolRoundLimit {
                        limit: self.max_tool_rounds,
                    };
                    return self.processing_failure(
                        exchange_id,
                        trace_context,
                        "tool_round_limit",
                        error,
                    );
                }
                let tool_responses = match workflow.tool_responses(&tool_calls) {
                    Ok(Some(tool_responses)) => tool_responses,
                    Ok(None) => {
                        let error = unexpected_finish_reason(&response, true);
                        return self.processing_failure(
                            exchange_id,
                            trace_context,
                            "unexpected_finish_reason",
                            error,
                        );
                    }
                    Err(error) => {
                        return self.processing_failure(
                            exchange_id,
                            trace_context,
                            "tool_execution",
                            error,
                        );
                    }
                };
                diagnostics.tool_rounds += 1;
                request = request
                    .append_message(tool_calls)
                    .append_message(tool_responses);
                if diagnostics.tool_rounds == self.max_tool_rounds {
                    request.tools = None;
                }
                conversation_turn += 1;
                request_kind = ModelRequestKind::ToolFollowUp;
                continue;
            }

            match response.stop_reason.as_ref() {
                Some(StopReason::Completed(_)) => {}
                Some(StopReason::MaxTokens(_)) => {
                    let error = ChatCompletionsError::OutputTruncated;
                    return self.processing_failure(
                        exchange_id,
                        trace_context,
                        "output_truncated",
                        error,
                    );
                }
                _ => {
                    let error = unexpected_finish_reason(&response, false);
                    return self.processing_failure(
                        exchange_id,
                        trace_context,
                        "unexpected_finish_reason",
                        error,
                    );
                }
            }
            let Some(content) = response
                .first_text()
                .filter(|content| !content.trim().is_empty())
            else {
                let error = ChatCompletionsError::MissingAssistantContent;
                return self.processing_failure(
                    exchange_id,
                    trace_context,
                    "missing_assistant_content",
                    error,
                );
            };
            match workflow.parse_and_validate(content) {
                Ok((output, accepted_json_fence)) => {
                    self.record_validation(exchange_id, trace_context, None)?;
                    diagnostics.accepted_json_fence = accepted_json_fence;
                    return Ok(ConversationOutcome {
                        output,
                        diagnostics,
                    });
                }
                Err(error) => {
                    self.record_validation(exchange_id, trace_context, Some(&error))?;
                    if diagnostics.final_answer_repairs == self.max_final_answer_repairs {
                        return Err(ChatCompletionsError::FinalAnswerRepairLimit {
                            limit: self.max_final_answer_repairs,
                            source: Box::new(error),
                        });
                    }
                    diagnostics.final_answer_repairs += 1;
                    request = request
                        .append_message(ChatMessage::assistant(content.to_owned()))
                        .append_message(ChatMessage::user(workflow.repair_instruction(&error)));
                    conversation_turn += 1;
                    request_kind = ModelRequestKind::Repair;
                }
            }
        }
    }

    async fn chat(
        &self,
        request: ChatRequest,
        tools_enabled: bool,
        trace_context: ModelTraceContext,
    ) -> Result<(ChatResponse, usize, Option<u64>), ChatCompletionsError> {
        let options = if tools_enabled {
            self.options.clone().with_tool_choice(ToolChoice::Auto)
        } else {
            self.options.clone()
        };
        let mut retries = 0;
        loop {
            self.wait_for_request_slot().await;
            let provider_attempt = retries;
            let exchange_id = self
                .model_trace
                .as_ref()
                .map(|trace| {
                    trace.record_request(
                        trace_context,
                        provider_attempt,
                        &self.endpoint,
                        &self.model,
                        &request,
                        &options,
                    )
                })
                .transpose()
                .map_err(ChatCompletionsError::ModelTrace)?;
            let started = Instant::now();
            match self
                .transport
                .exec_chat(self.target.clone(), request.clone(), Some(&options))
                .await
            {
                Ok(response) => {
                    if let (Some(trace), Some(exchange_id)) = (&self.model_trace, exchange_id) {
                        trace
                            .record_response(
                                exchange_id,
                                trace_context,
                                provider_attempt,
                                started.elapsed(),
                                &response,
                                response.captured_raw_body.clone(),
                            )
                            .map_err(ChatCompletionsError::ModelTrace)?;
                    }
                    return Ok((response, retries, exchange_id));
                }
                Err(error) => {
                    let retryable = is_retryable_provider_error(&error);
                    let will_retry = retries < self.max_provider_retries && retryable;
                    if let (Some(trace), Some(exchange_id)) = (&self.model_trace, exchange_id) {
                        trace
                            .record_provider_error(
                                exchange_id,
                                trace_context,
                                ModelProviderFailure {
                                    provider_attempt,
                                    elapsed: started.elapsed(),
                                    retryable,
                                    will_retry,
                                    error: model_provider_error(&error),
                                },
                            )
                            .map_err(ChatCompletionsError::ModelTrace)?;
                    }
                    if !will_retry {
                        return Err(provider_error(error));
                    }
                    retries += 1;
                    tokio::time::sleep(provider_retry_delay(&error, retries)).await;
                }
            }
        }
    }

    async fn wait_for_request_slot(&self) {
        if self.minimum_request_interval.is_zero() {
            return;
        }
        let now = Instant::now();
        let delay = {
            let mut next_request_at = self
                .next_request_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let scheduled_at = next_request_at.map_or(now, |next| next.max(now));
            *next_request_at = scheduled_at.checked_add(self.minimum_request_interval);
            scheduled_at.saturating_duration_since(now)
        };
        tokio::time::sleep(delay).await;
    }

    fn record_validation(
        &self,
        exchange_id: Option<u64>,
        context: ModelTraceContext,
        error: Option<&ChatCompletionsError>,
    ) -> Result<(), ChatCompletionsError> {
        let (Some(trace), Some(exchange_id)) = (&self.model_trace, exchange_id) else {
            return Ok(());
        };
        trace
            .record_validation(
                exchange_id,
                context,
                error.is_none(),
                error.map(trace_error_category),
                error.map(ToString::to_string).as_deref(),
            )
            .map_err(ChatCompletionsError::ModelTrace)
    }

    fn processing_failure<T>(
        &self,
        exchange_id: Option<u64>,
        context: ModelTraceContext,
        category: &str,
        error: ChatCompletionsError,
    ) -> Result<T, ChatCompletionsError> {
        if let (Some(trace), Some(exchange_id)) = (&self.model_trace, exchange_id) {
            trace
                .record_processing_error(exchange_id, context, category, &error.to_string())
                .map_err(ChatCompletionsError::ModelTrace)?;
        }
        Err(error)
    }
}

trait ConversationWorkflow {
    type Output;

    fn trace_context(
        &self,
        conversation_turn: usize,
        request_kind: ModelRequestKind,
    ) -> ModelTraceContext;

    fn tool_responses(
        &mut self,
        tool_calls: &[ToolCall],
    ) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError>;

    fn parse_and_validate(
        &self,
        content: &str,
    ) -> Result<(Self::Output, bool), ChatCompletionsError>;

    fn repair_instruction(&self, error: &ChatCompletionsError) -> String;
}

struct AnnotationWorkflow<'sources, 'task> {
    sources: &'sources ValidatedSources,
    task: TranscriptWindowTask<'task>,
    session: AnnotationToolSession<'sources>,
    max_search_results: usize,
}

impl<'sources, 'task> AnnotationWorkflow<'sources, 'task> {
    fn new(
        sources: &'sources ValidatedSources,
        scorer: &'sources dyn SlideScorer,
        task: &TranscriptWindowTask<'task>,
        max_search_results: usize,
    ) -> Self {
        Self {
            sources,
            task: *task,
            session: AnnotationToolSession::for_task(sources, scorer, task),
            max_search_results,
        }
    }
}

fn execute_annotation_tool_call(
    session: &mut AnnotationToolSession<'_>,
    max_search_results: usize,
    tool_call: &ToolCall,
) -> Result<String, ChatCompletionsError> {
    match tool_call.fn_name.as_str() {
        "inspect_slide" => {
            let arguments = match serde_json::from_value::<InspectSlideArguments>(
                tool_call.fn_arguments.clone(),
            ) {
                Ok(arguments) => arguments,
                Err(error) => return invalid_arguments("inspect_slide", error),
            };
            match session.inspect_slide(arguments.slide_id) {
                Ok(evidence) => {
                    serde_json::to_string(&evidence).map_err(ChatCompletionsError::SerializeTool)
                }
                Err(AnnotationToolError::UnknownSlide { slide }) => {
                    tool_error("unknown_slide", format!("slide {} does not exist", slide.0))
                }
                Err(error) => Err(ChatCompletionsError::Tool(error)),
            }
        }
        "search_slides" => {
            let arguments = match serde_json::from_value::<SearchSlidesArguments>(
                tool_call.fn_arguments.clone(),
            ) {
                Ok(arguments) => arguments,
                Err(error) => return invalid_arguments("search_slides", error),
            };
            if arguments.query.trim().is_empty() {
                return tool_error("invalid_arguments", "search query cannot be empty");
            }
            if arguments.max_results == 0 || arguments.max_results > max_search_results {
                return tool_error(
                    "invalid_arguments",
                    format!("max_results must be between 1 and {}", max_search_results),
                );
            }
            let evidence = session
                .search_slides(&arguments.query, arguments.max_results)
                .map_err(ChatCompletionsError::Tool)?;
            serde_json::to_string(&evidence).map_err(ChatCompletionsError::SerializeTool)
        }
        name => tool_error(
            "unknown_tool",
            format!("unknown tool {name:?}; expected inspect_slide or search_slides"),
        ),
    }
}

fn annotation_tool_responses(
    session: &mut AnnotationToolSession<'_>,
    max_search_results: usize,
    tool_calls: &[ToolCall],
) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError> {
    tool_calls
        .iter()
        .map(|tool_call| {
            execute_annotation_tool_call(session, max_search_results, tool_call)
                .map(|content| ToolResponse::from_tool_call(tool_call, content))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

impl ConversationWorkflow for AnnotationWorkflow<'_, '_> {
    type Output = TranscriptWindowAnalysis;

    fn trace_context(
        &self,
        conversation_turn: usize,
        request_kind: ModelRequestKind,
    ) -> ModelTraceContext {
        ModelTraceContext {
            workflow: ModelWorkflow::Annotation,
            work_item_index: self.task.window_number.saturating_sub(1),
            conversation_turn,
            request_kind,
        }
    }

    fn tool_responses(
        &mut self,
        tool_calls: &[ToolCall],
    ) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError> {
        annotation_tool_responses(&mut self.session, self.max_search_results, tool_calls)
    }

    fn parse_and_validate(
        &self,
        content: &str,
    ) -> Result<(Self::Output, bool), ChatCompletionsError> {
        parse_analysis(content).and_then(|(analysis, accepted_json_fence)| {
            validate_window_analysis(self.sources, &self.task, &analysis)
                .map_err(ChatCompletionsError::InvalidWindowAnalysis)?;
            Ok((analysis, accepted_json_fence))
        })
    }

    fn repair_instruction(&self, error: &ChatCompletionsError) -> String {
        analysis_repair_instruction(error)
    }
}

struct ProjectedAnnotation {
    analysis: RestoredTranscriptWindowAnalysis,
    projection: PassageProjectionDiagnostics,
}

struct RestoredAnnotationWorkflow<'sources, 'task> {
    sources: &'sources ValidatedSources,
    task: RestoredTranscriptWindowTask<'task>,
    session: AnnotationToolSession<'sources>,
    max_search_results: usize,
}

impl<'sources, 'task> RestoredAnnotationWorkflow<'sources, 'task> {
    fn new(
        sources: &'sources ValidatedSources,
        scorer: &'sources dyn SlideScorer,
        task: &RestoredTranscriptWindowTask<'task>,
        max_search_results: usize,
    ) -> Self {
        Self {
            sources,
            task: *task,
            session: AnnotationToolSession::for_nearby_slides(
                sources,
                scorer,
                task.nearby_slides(),
            ),
            max_search_results,
        }
    }
}

impl ConversationWorkflow for RestoredAnnotationWorkflow<'_, '_> {
    type Output = ProjectedAnnotation;

    fn trace_context(
        &self,
        conversation_turn: usize,
        request_kind: ModelRequestKind,
    ) -> ModelTraceContext {
        ModelTraceContext {
            workflow: ModelWorkflow::Annotation,
            work_item_index: self.task.window_index(),
            conversation_turn,
            request_kind,
        }
    }

    fn tool_responses(
        &mut self,
        tool_calls: &[ToolCall],
    ) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError> {
        annotation_tool_responses(&mut self.session, self.max_search_results, tool_calls)
    }

    fn parse_and_validate(
        &self,
        content: &str,
    ) -> Result<(Self::Output, bool), ChatCompletionsError> {
        parse_restored_analysis(content).and_then(|(proposed, accepted_json_fence)| {
            let (analysis, projection) =
                project_window_analysis(self.sources, &self.task, proposed)
                    .map_err(ChatCompletionsError::InvalidRestoredWindowAnalysis)?;
            Ok((
                ProjectedAnnotation {
                    analysis,
                    projection: projection.diagnostics(),
                },
                accepted_json_fence,
            ))
        })
    }

    fn repair_instruction(&self, error: &ChatCompletionsError) -> String {
        restored_analysis_repair_instruction(error)
    }
}

struct RestorationWorkflow<'task> {
    task: TranscriptRestorationTask<'task>,
}

impl ConversationWorkflow for RestorationWorkflow<'_> {
    type Output = TranscriptWindowRestoration;

    fn trace_context(
        &self,
        conversation_turn: usize,
        request_kind: ModelRequestKind,
    ) -> ModelTraceContext {
        ModelTraceContext {
            workflow: ModelWorkflow::Restoration,
            work_item_index: self.task.window_index,
            conversation_turn,
            request_kind,
        }
    }

    fn tool_responses(
        &mut self,
        _tool_calls: &[ToolCall],
    ) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError> {
        Ok(None)
    }

    fn parse_and_validate(
        &self,
        content: &str,
    ) -> Result<(Self::Output, bool), ChatCompletionsError> {
        parse_restoration(content).and_then(|(restoration, accepted_json_fence)| {
            validate_window_restoration(&self.task, &restoration)
                .map_err(ChatCompletionsError::InvalidWindowRestoration)?;
            Ok((restoration, accepted_json_fence))
        })
    }

    fn repair_instruction(&self, error: &ChatCompletionsError) -> String {
        restoration_repair_instruction(&self.task, error)
    }
}

struct ComparativeRankingWorkflow<'task> {
    task: &'task ComparativeRankingTask,
}

impl ConversationWorkflow for ComparativeRankingWorkflow<'_> {
    type Output = Vec<crate::ComparativeDecision>;

    fn trace_context(
        &self,
        conversation_turn: usize,
        request_kind: ModelRequestKind,
    ) -> ModelTraceContext {
        ModelTraceContext {
            workflow: match self.task.metric() {
                ComparativeMetric::Importance => ModelWorkflow::ImportanceComparison,
                ComparativeMetric::Novelty => ModelWorkflow::NoveltyComparison,
            },
            work_item_index: self.task.task_index(),
            conversation_turn,
            request_kind,
        }
    }

    fn tool_responses(
        &mut self,
        _tool_calls: &[ToolCall],
    ) -> Result<Option<Vec<ToolResponse>>, ChatCompletionsError> {
        Ok(None)
    }

    fn parse_and_validate(
        &self,
        content: &str,
    ) -> Result<(Self::Output, bool), ChatCompletionsError> {
        let (proposed, accepted_json_fence) =
            parse_json_content::<ProposedComparativeRanking>(content)
                .map_err(ChatCompletionsError::InvalidComparativeRankingJson)?;
        let decisions = self
            .task
            .validate(proposed)
            .map_err(ChatCompletionsError::InvalidComparativeRanking)?;
        Ok((decisions, accepted_json_fence))
    }

    fn repair_instruction(&self, error: &ChatCompletionsError) -> String {
        comparative_ranking_repair_instruction(error)
    }
}

struct ConversationOutcome<T> {
    output: T,
    diagnostics: ConversationDiagnostics,
}

struct ConversationDiagnostics {
    provider_retries: usize,
    tool_rounds: usize,
    final_answer_repairs: usize,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    accepted_json_fence: bool,
}

impl Default for ConversationDiagnostics {
    fn default() -> Self {
        Self {
            provider_retries: 0,
            tool_rounds: 0,
            final_answer_repairs: 0,
            prompt_tokens: Some(0),
            completion_tokens: Some(0),
            accepted_json_fence: false,
        }
    }
}

impl ConversationDiagnostics {
    fn record(&mut self, response: &ChatResponse, provider_retries: usize) {
        self.provider_retries = self.provider_retries.saturating_add(provider_retries);
        self.prompt_tokens = add_token_count(self.prompt_tokens, response.usage.prompt_tokens);
        self.completion_tokens =
            add_token_count(self.completion_tokens, response.usage.completion_tokens);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AnnotationResult {
    pub analysis: TranscriptWindowAnalysis,
    pub diagnostics: AnnotationDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RestoredAnnotationResult {
    pub analysis: RestoredTranscriptWindowAnalysis,
    pub diagnostics: AnnotationDiagnostics,
    pub projection: PassageProjectionDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranscriptWindowRestorationResult {
    pub restoration: TranscriptWindowRestoration,
    pub diagnostics: RestorationDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RestorationDiagnostics {
    /// Number of transient provider failures retried before accepted responses.
    #[serde(default)]
    pub provider_retries: usize,
    /// Number of schema or window-validation failures returned to the model for correction.
    pub final_answer_repairs: usize,
    /// Sum across every response, or `None` if any prompt-token count was absent or invalid.
    pub prompt_tokens: Option<u64>,
    /// Sum across every response, or `None` if any completion-token count was absent or invalid.
    pub completion_tokens: Option<u64>,
    /// True when a whole-response `json` code fence had to be removed.
    pub accepted_json_fence: bool,
}

impl Default for RestorationDiagnostics {
    fn default() -> Self {
        ConversationDiagnostics::default().into()
    }
}

impl From<ConversationDiagnostics> for RestorationDiagnostics {
    fn from(diagnostics: ConversationDiagnostics) -> Self {
        Self {
            provider_retries: diagnostics.provider_retries,
            final_answer_repairs: diagnostics.final_answer_repairs,
            prompt_tokens: diagnostics.prompt_tokens,
            completion_tokens: diagnostics.completion_tokens,
            accepted_json_fence: diagnostics.accepted_json_fence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AnnotationDiagnostics {
    /// Number of transient provider failures retried before accepted responses.
    #[serde(default)]
    pub provider_retries: usize,
    pub tool_rounds: usize,
    /// Number of schema or window-validation failures returned to the model for correction.
    pub final_answer_repairs: usize,
    /// Sum across every response, or `None` if any prompt-token count was absent or invalid.
    pub prompt_tokens: Option<u64>,
    /// Sum across every response, or `None` if any completion-token count was absent or invalid.
    pub completion_tokens: Option<u64>,
    /// True when a whole-response `json` code fence had to be removed.
    pub accepted_json_fence: bool,
}

impl Default for AnnotationDiagnostics {
    fn default() -> Self {
        ConversationDiagnostics::default().into()
    }
}

impl From<ConversationDiagnostics> for AnnotationDiagnostics {
    fn from(diagnostics: ConversationDiagnostics) -> Self {
        Self {
            provider_retries: diagnostics.provider_retries,
            tool_rounds: diagnostics.tool_rounds,
            final_answer_repairs: diagnostics.final_answer_repairs,
            prompt_tokens: diagnostics.prompt_tokens,
            completion_tokens: diagnostics.completion_tokens,
            accepted_json_fence: diagnostics.accepted_json_fence,
        }
    }
}

fn add_token_count(total: Option<u64>, count: Option<i32>) -> Option<u64> {
    total?.checked_add(u64::try_from(count?).ok()?)
}

#[derive(Debug)]
pub enum ChatCompletionsError {
    Provider(String),
    ModelTrace(io::Error),
    SerializeTask(serde_json::Error),
    UnexpectedChoiceCount {
        actual: usize,
    },
    UnexpectedFinishReason {
        reason: String,
        has_tool_calls: bool,
    },
    MissingAssistantContent,
    OutputTruncated,
    ToolRoundLimit {
        limit: usize,
    },
    FinalAnswerRepairLimit {
        limit: usize,
        source: Box<ChatCompletionsError>,
    },
    Tool(AnnotationToolError),
    SerializeTool(serde_json::Error),
    InvalidAnalysisJson(serde_json::Error),
    InvalidWindowAnalysis(AnalysisAssemblyError),
    InvalidRestoredAnalysisJson(serde_json::Error),
    InvalidRestoredWindowAnalysis(RestoredAnnotationError),
    InvalidRestorationJson(serde_json::Error),
    InvalidWindowRestoration(RestorationError),
    InvalidComparativeRankingJson(serde_json::Error),
    InvalidComparativeRanking(ComparativeRankingValidationError),
}

impl fmt::Display for ChatCompletionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => {
                write!(formatter, "the model endpoint request failed: {error}")
            }
            Self::ModelTrace(error) => write!(formatter, "could not record model exchange: {error}"),
            Self::SerializeTask(error) => write!(
                formatter,
                "could not serialize the model task: {error}"
            ),
            Self::UnexpectedChoiceCount { actual } => {
                write!(
                    formatter,
                    "the model endpoint returned {actual} choices instead of exactly one"
                )
            }
            Self::UnexpectedFinishReason {
                reason,
                has_tool_calls,
            } => write!(
                formatter,
                "the model finished with reason {reason:?} (tool calls present: {has_tool_calls})"
            ),
            Self::MissingAssistantContent => formatter
                .write_str("the model returned neither tool calls nor final assistant content"),
            Self::OutputTruncated => formatter.write_str(
                "the model exhausted the output-token limit before returning a complete structured response",
            ),
            Self::ToolRoundLimit { limit } => write!(
                formatter,
                "the model requested more than the configured {limit} tool-call rounds"
            ),
            Self::FinalAnswerRepairLimit { limit, source } => write!(
                formatter,
                "the model still returned an invalid structured response after {limit} repair attempts: {source}"
            ),
            Self::Tool(error) => write!(formatter, "annotation tool failed: {error}"),
            Self::SerializeTool(error) => {
                write!(
                    formatter,
                    "could not serialize an annotation tool result: {error}"
                )
            }
            Self::InvalidAnalysisJson(error) => {
                write!(
                    formatter,
                    "the model's final content is not a TranscriptWindowAnalysis JSON object: {error}"
                )
            }
            Self::InvalidWindowAnalysis(error) => {
                write!(
                    formatter,
                    "the model returned an invalid transcript-window analysis: {error}"
                )
            }
            Self::InvalidRestoredAnalysisJson(error) => write!(
                formatter,
                "the model's final content is not a ProposedTranscriptWindowAnalysis JSON object: {error}"
            ),
            Self::InvalidRestoredWindowAnalysis(error) => write!(
                formatter,
                "the model returned an invalid restored-transcript-window analysis: {error}"
            ),
            Self::InvalidRestorationJson(error) => write!(
                formatter,
                "the model's final content is not a TranscriptWindowRestoration JSON object: {error}"
            ),
            Self::InvalidWindowRestoration(error) => {
                write!(
                    formatter,
                    "the model returned an invalid transcript-window restoration: {error}"
                )
            }
            Self::InvalidComparativeRankingJson(error) => write!(
                formatter,
                "the model's final content is not a comparative-ranking JSON object: {error}"
            ),
            Self::InvalidComparativeRanking(error) => write!(
                formatter,
                "the model returned an invalid comparative-ranking result: {error}"
            ),
        }
    }
}

impl Error for ChatCompletionsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ModelTrace(error) => Some(error),
            Self::SerializeTask(error)
            | Self::SerializeTool(error)
            | Self::InvalidAnalysisJson(error)
            | Self::InvalidRestoredAnalysisJson(error)
            | Self::InvalidRestorationJson(error)
            | Self::InvalidComparativeRankingJson(error) => Some(error),
            Self::Tool(error) => Some(error),
            Self::InvalidWindowAnalysis(error) => Some(error),
            Self::InvalidRestoredWindowAnalysis(error) => Some(error),
            Self::InvalidWindowRestoration(error) => Some(error),
            Self::InvalidComparativeRanking(error) => Some(error),
            Self::FinalAnswerRepairLimit { source, .. } => Some(source.as_ref()),
            Self::Provider(_)
            | Self::UnexpectedChoiceCount { .. }
            | Self::UnexpectedFinishReason { .. }
            | Self::MissingAssistantContent
            | Self::OutputTruncated
            | Self::ToolRoundLimit { .. } => None,
        }
    }
}

fn is_retryable_provider_error(error: &genai::Error) -> bool {
    match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => is_retryable_web_error(webc_error),
        genai::Error::HttpError { status, .. } => is_retryable_status(status.as_u16()),
        _ => false,
    }
}

fn is_retryable_web_error(error: &genai::webc::Error) -> bool {
    match error {
        genai::webc::Error::ResponseFailedStatus { status, .. } => {
            is_retryable_status(status.as_u16())
        }
        genai::webc::Error::Reqwest(error) => {
            error.is_connect() || error.is_request() || error.is_timeout()
        }
        genai::webc::Error::ResponseFailedNotJson { .. }
        | genai::webc::Error::ResponseFailedInvalidJson { .. } => true,
        genai::webc::Error::JsonValueExt(_) => false,
    }
}

fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..=599).contains(&status)
}

fn provider_retry_delay(error: &genai::Error, retry: usize) -> Duration {
    if let Some(delay) = retry_after_delay(error) {
        return delay;
    }
    if provider_error_status(error) == Some(429) {
        let exponent = u32::try_from(retry.saturating_sub(1).min(4)).unwrap_or(4);
        return Duration::from_secs(5_u64.saturating_mul(2_u64.pow(exponent)));
    }
    Duration::from_secs(retry.min(5) as u64)
}

fn retry_after_delay(error: &genai::Error) -> Option<Duration> {
    let web_error = match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => webc_error,
        _ => return None,
    };
    let genai::webc::Error::ResponseFailedStatus { headers, .. } = web_error else {
        return None;
    };
    let seconds = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(seconds.min(120)))
}

fn provider_error_status(error: &genai::Error) -> Option<u16> {
    match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => match webc_error {
            genai::webc::Error::ResponseFailedStatus { status, .. } => Some(status.as_u16()),
            _ => None,
        },
        genai::Error::HttpError { status, .. } => Some(status.as_u16()),
        _ => None,
    }
}

fn model_provider_error(error: &genai::Error) -> ModelProviderError {
    match error {
        genai::Error::WebAdapterCall { webc_error, .. }
        | genai::Error::WebModelCall { webc_error, .. } => model_web_error(webc_error),
        genai::Error::HttpError { status, body, .. } => ModelProviderError {
            kind: "http_status".into(),
            message: error.to_string(),
            status: Some(status.as_u16()),
            body: Some(Value::String(body.clone())),
        },
        genai::Error::ChatResponseGeneration {
            response_body,
            cause,
            ..
        } => ModelProviderError {
            kind: "response_decode".into(),
            message: cause.clone(),
            status: None,
            body: Some(response_body.as_ref().clone()),
        },
        genai::Error::ChatResponse { body, .. } => ModelProviderError {
            kind: "provider_response".into(),
            message: error.to_string(),
            status: None,
            body: Some(body.clone()),
        },
        _ => ModelProviderError {
            kind: "provider".into(),
            message: error.to_string(),
            status: None,
            body: None,
        },
    }
}

fn model_web_error(error: &genai::webc::Error) -> ModelProviderError {
    match error {
        genai::webc::Error::ResponseFailedStatus { status, body, .. } => ModelProviderError {
            kind: "http_status".into(),
            message: error.to_string(),
            status: Some(status.as_u16()),
            body: Some(Value::String(body.clone())),
        },
        genai::webc::Error::ResponseFailedNotJson { body, .. } => ModelProviderError {
            kind: "non_json_response".into(),
            message: error.to_string(),
            status: None,
            body: Some(Value::String(body.clone())),
        },
        genai::webc::Error::ResponseFailedInvalidJson { body, .. } => ModelProviderError {
            kind: "invalid_json_response".into(),
            message: error.to_string(),
            status: None,
            body: Some(Value::String(body.clone())),
        },
        genai::webc::Error::Reqwest(_) => ModelProviderError {
            kind: "transport".into(),
            message: error.to_string(),
            status: None,
            body: None,
        },
        genai::webc::Error::JsonValueExt(_) => ModelProviderError {
            kind: "response_decode".into(),
            message: error.to_string(),
            status: None,
            body: None,
        },
    }
}

fn trace_error_category(error: &ChatCompletionsError) -> &'static str {
    match error {
        ChatCompletionsError::InvalidAnalysisJson(_)
        | ChatCompletionsError::InvalidRestoredAnalysisJson(_)
        | ChatCompletionsError::InvalidRestorationJson(_)
        | ChatCompletionsError::InvalidComparativeRankingJson(_) => "malformed_json",
        ChatCompletionsError::InvalidComparativeRanking(_) => "invalid_comparative_ranking",
        ChatCompletionsError::InvalidWindowRestoration(error) => restoration_error_category(error),
        ChatCompletionsError::InvalidWindowAnalysis(error) => analysis_error_category(error),
        ChatCompletionsError::InvalidRestoredWindowAnalysis(error) => {
            restored_analysis_error_category(error)
        }
        _ => "structured_response",
    }
}

fn restored_analysis_error_category(error: &RestoredAnnotationError) -> &'static str {
    match error {
        RestoredAnnotationError::PassageProjection(
            PassageProjectionError::DifferenceTooLarge { .. },
        ) => "passage_text_difference",
        RestoredAnnotationError::PassageProjection(PassageProjectionError::NoPassages) => {
            "no_passages"
        }
        RestoredAnnotationError::PassageProjection(
            PassageProjectionError::EmptyProposedPassage { .. }
            | PassageProjectionError::EmptyProjectedPassage { .. },
        ) => "empty_passage",
        RestoredAnnotationError::UnknownRelatedSlide { .. } => "unknown_related_slide",
        RestoredAnnotationError::DuplicateRelatedSlide { .. } => "duplicate_related_slide",
        RestoredAnnotationError::SlidePositionCountMismatch { .. }
        | RestoredAnnotationError::EmptyOwnedText { .. }
        | RestoredAnnotationError::UnknownSlidePosition { .. }
        | RestoredAnnotationError::UnknownPassageSlidePosition { .. }
        | RestoredAnnotationError::PassageSlidePositionMismatch { .. }
        | RestoredAnnotationError::MissingSourceProvenance { .. }
        | RestoredAnnotationError::EmptyTrustedPassage { .. }
        | RestoredAnnotationError::TrustedTextMismatch { .. }
        | RestoredAnnotationError::UncoveredOwnedText { .. }
        | RestoredAnnotationError::SourceProvenanceMismatch { .. }
        | RestoredAnnotationError::IncompleteComparativeScores { .. }
        | RestoredAnnotationError::InvalidComparativeScore { .. }
        | RestoredAnnotationError::ComparativeDisplayLevelMismatch { .. }
        | RestoredAnnotationError::PassageProjection(
            PassageProjectionError::EmptySource
            | PassageProjectionError::NonMonotonicProjection { .. },
        ) => "invalid_restored_analysis",
    }
}

fn restoration_error_category(error: &RestorationError) -> &'static str {
    match error {
        RestorationError::WindowCountMismatch { .. } => "window_count",
        RestorationError::EmptyOwnedRegion { .. } => "empty_owned_region",
        RestorationError::SpanEndBeforeStart { .. } => "reversed_range",
        RestorationError::SpanOutsideOwnedRegion { .. } => "outside_owned_region",
        RestorationError::CoverageMismatch {
            expected, actual, ..
        } if actual.0 > expected.0 => "coverage_gap",
        RestorationError::CoverageMismatch { .. } => "coverage_overlap",
        RestorationError::UnexpectedSpan { .. } => "unexpected_range",
        RestorationError::UncoveredTail { .. } => "uncovered_tail",
        RestorationError::EmptyText { .. } => "empty_text",
    }
}

fn analysis_error_category(error: &AnalysisAssemblyError) -> &'static str {
    match error {
        AnalysisAssemblyError::WindowCountMismatch { .. } => "window_count",
        AnalysisAssemblyError::EmptyOwnedRegion { .. } => "empty_owned_region",
        AnalysisAssemblyError::PassageOutsideOwnedRegion { .. } => "outside_owned_region",
        AnalysisAssemblyError::WindowCoverageMismatch {
            expected, actual, ..
        } if actual.0 > expected.0 => "coverage_gap",
        AnalysisAssemblyError::WindowCoverageMismatch { .. } => "coverage_overlap",
        AnalysisAssemblyError::UnexpectedWindowPassage { .. } => "unexpected_range",
        AnalysisAssemblyError::UncoveredWindowTail { .. } => "uncovered_tail",
        AnalysisAssemblyError::InvalidAnalysis(_) => "invalid_analysis",
    }
}

fn provider_error(error: genai::Error) -> ChatCompletionsError {
    let message = match error {
        genai::Error::ChatResponseGeneration { cause, .. } => {
            format!("the response could not be decoded: {cause}")
        }
        error => error.to_string(),
    };
    ChatCompletionsError::Provider(
        message
            .chars()
            .take(MAX_PROVIDER_ERROR_CHARACTERS)
            .collect(),
    )
}

fn ensure_one_choice(response: &ChatResponse) -> Result<(), ChatCompletionsError> {
    let actual = response
        .captured_raw_body
        .as_ref()
        .and_then(|body| body.get("choices"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if actual == 1 {
        Ok(())
    } else {
        Err(ChatCompletionsError::UnexpectedChoiceCount { actual })
    }
}

fn unexpected_finish_reason(response: &ChatResponse, has_tool_calls: bool) -> ChatCompletionsError {
    ChatCompletionsError::UnexpectedFinishReason {
        reason: response
            .stop_reason
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<missing>".into()),
        has_tool_calls,
    }
}

fn parse_analysis(content: &str) -> Result<(TranscriptWindowAnalysis, bool), ChatCompletionsError> {
    parse_json_content(content).map_err(ChatCompletionsError::InvalidAnalysisJson)
}

fn parse_restored_analysis(
    content: &str,
) -> Result<(ProposedTranscriptWindowAnalysis, bool), ChatCompletionsError> {
    parse_json_content(content).map_err(ChatCompletionsError::InvalidRestoredAnalysisJson)
}

fn parse_restoration(
    content: &str,
) -> Result<(TranscriptWindowRestoration, bool), ChatCompletionsError> {
    parse_json_content(content).map_err(ChatCompletionsError::InvalidRestorationJson)
}

fn parse_json_content<T: DeserializeOwned>(content: &str) -> Result<(T, bool), serde_json::Error> {
    let content = content.trim();
    match serde_json::from_str(content) {
        Ok(value) => Ok((value, false)),
        Err(direct_error) => {
            let Some(payload) = strip_json_fence(content) else {
                return Err(direct_error);
            };
            serde_json::from_str(payload).map(|value| (value, true))
        }
    }
}

fn analysis_repair_instruction(error: &ChatCompletionsError) -> String {
    format!(
        "你上一条最终答案未通过验证：{error}\n请返回修正后的完整 TranscriptWindowAnalysis JSON 对象，不要只返回局部修改，也不要调用工具。每个 passage 都必须包含全部必需字段；如果没有相关幻灯片，related_slides 使用空数组 []。"
    )
}

fn restored_analysis_repair_instruction(error: &ChatCompletionsError) -> String {
    format!(
        "你上一条最终答案未通过验证：{error}\n请返回修正后的完整 ProposedTranscriptWindowAnalysis JSON 对象，不要调用工具。所有 passage.text 按顺序直接拼接后必须与 owned_text 逐字符完全相同；不得修正文句、空白或代码。每个 passage 都必须包含全部必需字段；如果没有相关幻灯片，related_slides 使用空数组 []。"
    )
}

fn restoration_repair_instruction(
    task: &TranscriptRestorationTask<'_>,
    error: &ChatCompletionsError,
) -> String {
    let owned_range = match (task.owned_region.first(), task.owned_region.last()) {
        (Some(first), Some(last)) => format!(
            "本窗口只允许讲座全局来源 ID {} 至 {}。第一个 span 的 source_start 必须是 {}，最后一个 span 的 source_end 必须是 {}。",
            first.id.0, last.id.0, first.id.0, last.id.0
        ),
        _ => "本窗口的 owned_region 为空，不得输出任何 span。".into(),
    };
    format!(
        "你上一条最终答案未通过验证：{error}\n请丢弃上一答案，从原始输入的 owned_region 重新生成完整 TranscriptWindowRestoration JSON 对象；不要平移或局部修补上一答案的 ID。{owned_range} source_start 和 source_end 必须直接复制 owned_region 中的 segment.id，不得按窗口位置从 0 重新编号。不得输出 left_context 或 right_context 的文字，即使句子因此不完整。spans 必须按顺序、无重叠、无遗漏地完整划分 owned_region；不要输出 Markdown 或解释。"
    )
}

fn comparative_ranking_repair_instruction(error: &ChatCompletionsError) -> String {
    format!(
        "你上一条最终答案未通过验证：{error}\n请返回修正后的完整比较 JSON 对象，不要只返回局部修改。必须恰好包含输入中的每个 comparison_id 一次；most 和 least 必须是该组 candidates 中实际存在的不同标签（A、B、C、D），不得返回段落编号或引用其他组。不要输出 Markdown 或解释。"
    )
}

fn strip_json_fence(content: &str) -> Option<&str> {
    let content = content.strip_prefix("```json")?;
    let content = content
        .strip_prefix("\r\n")
        .or_else(|| content.strip_prefix('\n'))?;
    let content = content.strip_suffix("```")?;
    Some(content.trim())
}

fn invalid_arguments(
    tool: &'static str,
    error: serde_json::Error,
) -> Result<String, ChatCompletionsError> {
    tool_error(
        "invalid_arguments",
        format!("invalid {tool} arguments: {error}"),
    )
}

fn tool_error(
    code: &'static str,
    message: impl Into<String>,
) -> Result<String, ChatCompletionsError> {
    serde_json::to_string(&ToolErrorMessage {
        status: "error",
        code,
        message: message.into(),
    })
    .map_err(ChatCompletionsError::SerializeTool)
}

fn tool_definitions(max_search_results: usize) -> [Tool; 2] {
    [
        Tool::new("inspect_slide")
            .with_description("读取指定幻灯片；同一会话中已经可见的幻灯片不会重复返回正文。")
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "slide_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "从零开始的幻灯片 ID"
                    }
                },
                "required": ["slide_id"],
                "additionalProperties": false
            })),
        Tool::new("search_slides")
            .with_description(
                "在整套幻灯片中检索与查询相关的页面；不会重复返回会话中已经可见的正文。",
            )
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "description": "用于检索幻灯片的中文查询"
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": max_search_results
                    }
                },
                "required": ["query", "max_results"],
                "additionalProperties": false
            })),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectSlideArguments {
    slide_id: SlideId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchSlidesArguments {
    query: String,
    max_results: usize,
}

#[derive(Serialize)]
struct ToolErrorMessage {
    status: &'static str,
    code: &'static str,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANALYSIS: &str = r#"{
        "passages": [{
            "start": 0,
            "end": 0,
            "novelty": 1,
            "connection_strength": 2,
            "importance": 3,
            "related_slides": []
        }]
    }"#;

    #[test]
    fn final_analysis_accepts_direct_json_or_one_whole_json_fence() {
        let (_, direct_fence) = parse_analysis(ANALYSIS).expect("direct analysis JSON");
        let (_, wrapped_fence) = parse_analysis(&format!("```json\n{ANALYSIS}\n```"))
            .expect("whole-response JSON fence");

        assert!(!direct_fence);
        assert!(wrapped_fence);
    }

    #[test]
    fn final_analysis_does_not_extract_json_from_prose() {
        let error = parse_analysis(&format!("Here is the analysis:\n{ANALYSIS}"))
            .expect_err("JSON surrounded by prose must be rejected");

        assert!(matches!(
            error,
            ChatCompletionsError::InvalidAnalysisJson(_)
        ));
    }

    #[test]
    fn only_transient_http_statuses_are_retried() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(504));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
    }
}
