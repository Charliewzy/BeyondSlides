use std::{error::Error, fmt, time::Duration};

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
    SlideId, SlideScorer, ValidatedSources,
    annotation::{
        AnalysisAssemblyError, AnnotationToolError, AnnotationToolSession,
        TranscriptWindowAnalysis, TranscriptWindowTask, validate_window_analysis,
    },
    restoration::{
        RestorationError, TranscriptRestorationTask, TranscriptWindowRestoration,
        validate_window_restoration,
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
    extra_body: Option<Value>,
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
            extra_body: None,
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
}

impl ChatCompletionsClient {
    pub fn new(config: ChatCompletionsConfig) -> Self {
        let target = ServiceTarget {
            endpoint: Endpoint::from_owned(config.base_url.to_string()),
            auth: AuthData::from_single(config.api_key),
            model: ModelIden::new(AdapterKind::OpenAI, config.model),
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
        let instructions = format!(
            "{}\n\n工具调用预算：最多可以进行 {} 轮工具调用。一轮可以同时调用多个工具。收到最后一轮的工具结果后，必须直接返回最终 JSON，不得继续调用工具。",
            message.instructions, self.max_tool_rounds
        );
        let mut request = ChatRequest::from_user(message.input)
            .with_system(instructions)
            .with_tools(tool_definitions(self.max_search_results));
        let mut session = AnnotationToolSession::for_task(sources, scorer, task);
        let mut diagnostics = AnnotationDiagnostics::default();

        loop {
            let (response, provider_retries) = self.chat(request.clone(), true).await?;
            ensure_one_choice(&response)?;
            diagnostics.record(&response, provider_retries);
            let tool_calls: Vec<_> = response.tool_calls().into_iter().cloned().collect();

            if !tool_calls.is_empty() {
                if !matches!(response.stop_reason.as_ref(), Some(StopReason::ToolCall(_))) {
                    return Err(unexpected_finish_reason(&response, true));
                }
                if diagnostics.tool_rounds == self.max_tool_rounds {
                    return Err(ChatCompletionsError::ToolRoundLimit {
                        limit: self.max_tool_rounds,
                    });
                }
                diagnostics.tool_rounds += 1;

                let tool_responses = tool_calls
                    .iter()
                    .map(|tool_call| {
                        self.execute_tool_call(&mut session, tool_call)
                            .map(|content| ToolResponse::from_tool_call(tool_call, content))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                request = request
                    .append_message(tool_calls)
                    .append_message(tool_responses);
                continue;
            }

            match response.stop_reason.as_ref() {
                Some(StopReason::Completed(_)) => {}
                Some(StopReason::MaxTokens(_)) => {
                    return Err(ChatCompletionsError::OutputTruncated);
                }
                _ => return Err(unexpected_finish_reason(&response, false)),
            }
            let content = response
                .first_text()
                .filter(|content| !content.trim().is_empty())
                .ok_or(ChatCompletionsError::MissingAssistantContent)?;
            let candidate = parse_analysis(content).and_then(|(analysis, accepted_json_fence)| {
                validate_window_analysis(sources, task, &analysis)
                    .map_err(ChatCompletionsError::InvalidWindowAnalysis)?;
                Ok((analysis, accepted_json_fence))
            });
            match candidate {
                Ok((analysis, accepted_json_fence)) => {
                    diagnostics.accepted_json_fence = accepted_json_fence;
                    return Ok(AnnotationResult {
                        analysis,
                        diagnostics,
                    });
                }
                Err(error) => {
                    if diagnostics.final_answer_repairs == self.max_final_answer_repairs {
                        return Err(ChatCompletionsError::FinalAnswerRepairLimit {
                            limit: self.max_final_answer_repairs,
                            source: Box::new(error),
                        });
                    }
                    diagnostics.final_answer_repairs += 1;
                    request = request
                        .append_message(ChatMessage::assistant(content.to_owned()))
                        .append_message(ChatMessage::user(repair_instruction(&error)));
                }
            }
        }
    }

    /// Restores one transcript window without exposing annotation tools.
    pub async fn restore_window(
        &self,
        task: &TranscriptRestorationTask<'_>,
    ) -> Result<TranscriptWindowRestorationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let mut request = ChatRequest::from_user(message.input).with_system(message.instructions);
        let mut diagnostics = RestorationDiagnostics::default();

        loop {
            let (response, provider_retries) = self.chat(request.clone(), false).await?;
            ensure_one_choice(&response)?;
            diagnostics.record(&response, provider_retries);
            let has_tool_calls = response.tool_calls().into_iter().next().is_some();
            if has_tool_calls {
                return Err(unexpected_finish_reason(&response, true));
            }

            match response.stop_reason.as_ref() {
                Some(StopReason::Completed(_)) => {}
                Some(StopReason::MaxTokens(_)) => {
                    return Err(ChatCompletionsError::OutputTruncated);
                }
                _ => return Err(unexpected_finish_reason(&response, false)),
            }
            let content = response
                .first_text()
                .filter(|content| !content.trim().is_empty())
                .ok_or(ChatCompletionsError::MissingAssistantContent)?;
            let candidate =
                parse_restoration(content).and_then(|(restoration, accepted_json_fence)| {
                    validate_window_restoration(task, &restoration)
                        .map_err(ChatCompletionsError::InvalidWindowRestoration)?;
                    Ok((restoration, accepted_json_fence))
                });
            match candidate {
                Ok((restoration, accepted_json_fence)) => {
                    diagnostics.accepted_json_fence = accepted_json_fence;
                    return Ok(TranscriptWindowRestorationResult {
                        restoration,
                        diagnostics,
                    });
                }
                Err(error) => {
                    if diagnostics.final_answer_repairs == self.max_final_answer_repairs {
                        return Err(ChatCompletionsError::FinalAnswerRepairLimit {
                            limit: self.max_final_answer_repairs,
                            source: Box::new(error),
                        });
                    }
                    diagnostics.final_answer_repairs += 1;
                    request = request
                        .append_message(ChatMessage::assistant(content.to_owned()))
                        .append_message(ChatMessage::user(restoration_repair_instruction(&error)));
                }
            }
        }
    }

    async fn chat(
        &self,
        request: ChatRequest,
        tools_enabled: bool,
    ) -> Result<(ChatResponse, usize), ChatCompletionsError> {
        let options = if tools_enabled {
            self.options.clone().with_tool_choice(ToolChoice::Auto)
        } else {
            self.options.clone()
        };
        let mut retries = 0;
        loop {
            match self
                .transport
                .exec_chat(self.target.clone(), request.clone(), Some(&options))
                .await
            {
                Ok(response) => return Ok((response, retries)),
                Err(error)
                    if retries < self.max_provider_retries
                        && is_retryable_provider_error(&error) =>
                {
                    retries += 1;
                    tokio::time::sleep(provider_retry_delay(retries)).await;
                }
                Err(error) => return Err(provider_error(error)),
            }
        }
    }

    fn execute_tool_call(
        &self,
        session: &mut AnnotationToolSession<'_>,
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
                    Ok(evidence) => serde_json::to_string(&evidence)
                        .map_err(ChatCompletionsError::SerializeTool),
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
                if arguments.max_results == 0 || arguments.max_results > self.max_search_results {
                    return tool_error(
                        "invalid_arguments",
                        format!(
                            "max_results must be between 1 and {}",
                            self.max_search_results
                        ),
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AnnotationResult {
    pub analysis: TranscriptWindowAnalysis,
    pub diagnostics: AnnotationDiagnostics,
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
        Self {
            provider_retries: 0,
            final_answer_repairs: 0,
            prompt_tokens: Some(0),
            completion_tokens: Some(0),
            accepted_json_fence: false,
        }
    }
}

impl RestorationDiagnostics {
    fn record(&mut self, response: &ChatResponse, provider_retries: usize) {
        self.provider_retries = self.provider_retries.saturating_add(provider_retries);
        self.prompt_tokens = add_token_count(self.prompt_tokens, response.usage.prompt_tokens);
        self.completion_tokens =
            add_token_count(self.completion_tokens, response.usage.completion_tokens);
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

impl AnnotationDiagnostics {
    fn record(&mut self, response: &ChatResponse, provider_retries: usize) {
        self.provider_retries = self.provider_retries.saturating_add(provider_retries);
        self.prompt_tokens = add_token_count(self.prompt_tokens, response.usage.prompt_tokens);
        self.completion_tokens =
            add_token_count(self.completion_tokens, response.usage.completion_tokens);
    }
}

fn add_token_count(total: Option<u64>, count: Option<i32>) -> Option<u64> {
    total?.checked_add(u64::try_from(count?).ok()?)
}

#[derive(Debug)]
pub enum ChatCompletionsError {
    Provider(String),
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
    InvalidRestorationJson(serde_json::Error),
    InvalidWindowRestoration(RestorationError),
}

impl fmt::Display for ChatCompletionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => {
                write!(formatter, "the model endpoint request failed: {error}")
            }
            Self::SerializeTask(error) => write!(
                formatter,
                "could not serialize the transcript-window task: {error}"
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
        }
    }
}

impl Error for ChatCompletionsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SerializeTask(error)
            | Self::SerializeTool(error)
            | Self::InvalidAnalysisJson(error)
            | Self::InvalidRestorationJson(error) => Some(error),
            Self::Tool(error) => Some(error),
            Self::InvalidWindowAnalysis(error) => Some(error),
            Self::InvalidWindowRestoration(error) => Some(error),
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

fn provider_retry_delay(retry: usize) -> Duration {
    Duration::from_secs(retry.min(5) as u64)
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

fn repair_instruction(error: &ChatCompletionsError) -> String {
    format!(
        "你上一条最终答案未通过验证：{error}\n请返回修正后的完整 TranscriptWindowAnalysis JSON 对象，不要只返回局部修改，也不要调用工具。每个 passage 都必须包含全部必需字段；如果没有相关幻灯片，related_slides 使用空数组 []。"
    )
}

fn restoration_repair_instruction(error: &ChatCompletionsError) -> String {
    format!(
        "你上一条最终答案未通过验证：{error}\n请返回修正后的完整 TranscriptWindowRestoration JSON 对象，不要只返回局部修改。spans 必须按顺序、无重叠、无遗漏地完整划分 owned_region；不要输出 Markdown 或解释。"
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
