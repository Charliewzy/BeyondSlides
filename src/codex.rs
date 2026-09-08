//! Local Codex app-server backend using the user's cached ChatGPT sign-in.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, mpsc, oneshot},
};

use crate::{
    AnnotationDiagnostics, AnnotationResult, BoundaryBatchResult, BoundaryBatchTask,
    ChatCompletionsError, ComparativeMetric, ComparativeRankingBatchResult, LectureModelBackend,
    ModelExchangeTrace, ModelRequestKind, ModelWorkflow, PASSAGE_BOUNDARY_INSTRUCTIONS,
    RequestScheduler, RestorationDiagnostics, RestoredAnnotationResult,
    RestoredTranscriptWindowTask, SlideScorer, TranscriptRestorationTask,
    TranscriptWindowRestorationResult, TranscriptWindowTask, ValidatedSources,
    annotation::validate_window_analysis,
    chat_completions::{
        is_passage_text_difference, parse_analysis, parse_restoration, parse_restored_analysis,
    },
    comparative_ranking::{ComparativeRankingTask, ProposedComparativeRanking},
    model_trace::{ModelProviderFailure, ModelTraceContext},
    request_scheduling::RequestFeedback,
    restoration::validate_window_restoration,
    restored_annotation::project_window_analysis,
};

const DEFAULT_MAX_FINAL_ANSWER_REPAIRS: usize = 2;
const DEFAULT_MAX_FRESH_ANNOTATION_RETRIES: usize = 5;
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const ENDPOINT: &str = "codex-app-server://stdio";

/// Configuration for a local Codex CLI authenticated through `codex login`.
pub struct CodexAppServerConfig {
    executable: PathBuf,
    model: String,
    max_final_answer_repairs: usize,
    max_fresh_annotation_retries: usize,
    turn_timeout: Duration,
    request_scheduler: RequestScheduler,
    model_trace: Option<ModelExchangeTrace>,
}

impl CodexAppServerConfig {
    pub fn new(model: impl Into<String>) -> Result<Self, CodexAppServerConfigError> {
        let model = model.into();
        if model.trim().is_empty() {
            return Err(CodexAppServerConfigError::EmptyModel);
        }
        Ok(Self {
            executable: PathBuf::from("codex"),
            model,
            max_final_answer_repairs: DEFAULT_MAX_FINAL_ANSWER_REPAIRS,
            max_fresh_annotation_retries: DEFAULT_MAX_FRESH_ANNOTATION_RETRIES,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            request_scheduler: RequestScheduler::fixed(std::num::NonZeroUsize::MAX, Duration::ZERO),
            model_trace: None,
        })
    }

    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = executable.into();
        self
    }

    pub fn with_request_scheduler(mut self, scheduler: RequestScheduler) -> Self {
        self.request_scheduler = scheduler;
        self
    }

    pub fn with_model_trace(mut self, trace: ModelExchangeTrace) -> Self {
        self.model_trace = Some(trace);
        self
    }

    pub const fn with_turn_timeout(mut self, timeout: Duration) -> Self {
        self.turn_timeout = timeout;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAppServerConfigError {
    EmptyModel,
}

impl fmt::Display for CodexAppServerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModel => formatter.write_str("the Codex model name cannot be empty"),
        }
    }
}

impl Error for CodexAppServerConfigError {}

/// A lazily started, shared Codex app-server process.
///
/// Each task receives an ephemeral thread. The single stdio connection
/// multiplexes concurrent threads while preserving the pipeline's own
/// concurrency and checkpoint controls.
pub struct CodexAppServerClient {
    config: CodexAppServerConfig,
    server: Mutex<Option<Arc<AppServer>>>,
}

struct StructuredTask<'a> {
    instructions: &'a str,
    input: String,
    schema: Value,
    workflow: ModelWorkflow,
    work_item_index: usize,
    max_repairs: usize,
}

impl CodexAppServerClient {
    pub fn new(config: CodexAppServerConfig) -> Self {
        Self {
            config,
            server: Mutex::new(None),
        }
    }

    async fn server(&self) -> Result<Arc<AppServer>, ChatCompletionsError> {
        let mut server = self.server.lock().await;
        if let Some(server) = server.as_ref() {
            return Ok(Arc::clone(server));
        }
        let launched = AppServer::launch(&self.config.executable)
            .await
            .map_err(codex_error)?;
        *server = Some(Arc::clone(&launched));
        Ok(launched)
    }

    async fn run_structured<T>(
        &self,
        task: StructuredTask<'_>,
        parse: impl Fn(&str) -> Result<(T, bool), ChatCompletionsError>,
        repair: impl Fn(&ChatCompletionsError) -> String,
    ) -> Result<(T, CodexDiagnostics), ChatCompletionsError> {
        let server = self.server().await?;
        let thread_id = server
            .start_thread(&self.config.model)
            .await
            .map_err(codex_error)?;
        let mut events = server.subscribe(&thread_id).map_err(codex_error)?;
        let mut prompt = format!(
            "<instructions>\n{}\n</instructions>\n\n<lecture_data>\n{}\n</lecture_data>",
            task.instructions, task.input,
        );
        let mut diagnostics = CodexDiagnostics::default();

        for conversation_turn in 0..=task.max_repairs {
            let request_kind = if conversation_turn == 0 {
                ModelRequestKind::Initial
            } else {
                ModelRequestKind::Repair
            };
            let context = ModelTraceContext {
                workflow: task.workflow,
                work_item_index: task.work_item_index,
                conversation_turn,
                request_kind,
            };
            let permit = self.config.request_scheduler.acquire().await;
            let request = json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}],
                "model": self.config.model,
                "approvalPolicy": "never",
                "sandboxPolicy": {"type": "readOnly"},
                "outputSchema": task.schema,
            });
            let exchange_id = self
                .config
                .model_trace
                .as_ref()
                .map(|trace| {
                    trace.record_request(
                        context,
                        0,
                        ENDPOINT,
                        &self.config.model,
                        &request,
                        &json!({"transport": "app-server", "sandbox": "read-only"}),
                    )
                })
                .transpose()
                .map_err(ChatCompletionsError::ModelTrace)?;
            let started = Instant::now();
            let turn = server
                .turn(&thread_id, request, &mut events, self.config.turn_timeout)
                .await;
            match turn {
                Ok(turn) => {
                    permit.finish(RequestFeedback::Success);
                    diagnostics.prompt_tokens =
                        add_optional(diagnostics.prompt_tokens, turn.input_tokens);
                    diagnostics.completion_tokens =
                        add_optional(diagnostics.completion_tokens, turn.output_tokens);
                    if let (Some(trace), Some(exchange_id)) =
                        (&self.config.model_trace, exchange_id)
                    {
                        trace
                            .record_response(
                                exchange_id,
                                context,
                                0,
                                started.elapsed(),
                                &json!({
                                    "content": turn.content,
                                    "usage": {
                                        "prompt_tokens": turn.input_tokens,
                                        "completion_tokens": turn.output_tokens,
                                    },
                                }),
                                None,
                            )
                            .map_err(ChatCompletionsError::ModelTrace)?;
                    }
                    match parse(&turn.content) {
                        Ok((output, accepted_json_fence)) => {
                            if let (Some(trace), Some(exchange_id)) =
                                (&self.config.model_trace, exchange_id)
                            {
                                trace
                                    .record_validation(exchange_id, context, true, None, None)
                                    .map_err(ChatCompletionsError::ModelTrace)?;
                            }
                            diagnostics.accepted_json_fence = accepted_json_fence;
                            server.unsubscribe(&thread_id).await;
                            return Ok((output, diagnostics));
                        }
                        Err(error) => {
                            if let (Some(trace), Some(exchange_id)) =
                                (&self.config.model_trace, exchange_id)
                            {
                                trace
                                    .record_validation(
                                        exchange_id,
                                        context,
                                        false,
                                        Some("structured_validation"),
                                        Some(&error.to_string()),
                                    )
                                    .map_err(ChatCompletionsError::ModelTrace)?;
                            }
                            if conversation_turn == task.max_repairs {
                                server.unsubscribe(&thread_id).await;
                                return Err(ChatCompletionsError::FinalAnswerRepairLimit {
                                    limit: task.max_repairs,
                                    source: Box::new(error),
                                });
                            }
                            diagnostics.final_answer_repairs += 1;
                            prompt = repair(&error);
                        }
                    }
                }
                Err(error) => {
                    permit.finish(RequestFeedback::Failed { retry_after: None });
                    if let (Some(trace), Some(exchange_id)) =
                        (&self.config.model_trace, exchange_id)
                    {
                        trace
                            .record_provider_error(
                                exchange_id,
                                context,
                                ModelProviderFailure {
                                    provider_attempt: 0,
                                    elapsed: started.elapsed(),
                                    retryable: false,
                                    will_retry: false,
                                    error: crate::ModelProviderError {
                                        kind: "codex_app_server".into(),
                                        message: error.clone(),
                                        status: None,
                                        body: None,
                                    },
                                },
                            )
                            .map_err(ChatCompletionsError::ModelTrace)?;
                    }
                    server.unsubscribe(&thread_id).await;
                    return Err(codex_error(error));
                }
            }
        }
        unreachable!("the bounded repair loop always returns")
    }
}

#[async_trait]
impl LectureModelBackend for CodexAppServerClient {
    async fn annotate_window(
        &self,
        sources: &ValidatedSources,
        _scorer: &dyn SlideScorer,
        task: &TranscriptWindowTask<'_>,
    ) -> Result<AnnotationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let (analysis, diagnostics) = self
            .run_structured(
                StructuredTask {
                    instructions: message.instructions,
                    input: message.input,
                    schema: annotation_schema(),
                    workflow: ModelWorkflow::Annotation,
                    work_item_index: task.window_number.saturating_sub(1),
                    max_repairs: self.config.max_final_answer_repairs,
                },
                |content| {
                    parse_analysis(content).and_then(|(analysis, fence)| {
                        validate_window_analysis(sources, task, &analysis)
                            .map_err(ChatCompletionsError::InvalidWindowAnalysis)?;
                        Ok((analysis, fence))
                    })
                },
                |error| format!("上一份 JSON 无效：{error}。请重新阅读原始任务，只返回完整、有效的指定 JSON。"),
            )
            .await?;
        Ok(AnnotationResult {
            analysis,
            diagnostics: diagnostics.annotation(),
        })
    }

    async fn annotate_restored_window(
        &self,
        sources: &ValidatedSources,
        _scorer: &dyn SlideScorer,
        task: &RestoredTranscriptWindowTask<'_>,
    ) -> Result<RestoredAnnotationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let attempts = self.config.max_fresh_annotation_retries.saturating_add(1);
        for attempt in 0..attempts {
            let instructions = if attempt == 0 {
                message.instructions.to_owned()
            } else {
                format!(
                    "{}\n\n这是第 {}/{} 次全新尝试。上一尝试遗漏或改动了原文。重新逐字阅读 owned_text；所有 passage.text 按顺序拼接后必须与 owned_text 完全一致。",
                    message.instructions,
                    attempt + 1,
                    attempts,
                )
            };
            let result = self
                .run_structured(
                    StructuredTask {
                        instructions: &instructions,
                        input: message.input.clone(),
                        schema: restored_annotation_schema(),
                        workflow: ModelWorkflow::Annotation,
                        work_item_index: task.window_index(),
                        max_repairs: if attempt == 0 {
                            self.config.max_final_answer_repairs
                        } else {
                            0
                        },
                    },
                    |content| {
                        parse_restored_analysis(content).and_then(|(proposed, fence)| {
                            let (analysis, projection) =
                                project_window_analysis(sources, task, proposed)
                                    .map_err(ChatCompletionsError::InvalidRestoredWindowAnalysis)?;
                            Ok(((analysis, projection.diagnostics()), fence))
                        })
                    },
                    |error| format!("上一份 JSON 无效：{error}。请保持 owned_text 逐字不变，只返回完整、有效的指定 JSON。"),
                )
                .await;
            match result {
                Ok(((analysis, projection), diagnostics)) => {
                    return Ok(RestoredAnnotationResult {
                        analysis,
                        diagnostics: diagnostics.annotation(),
                        projection,
                    });
                }
                Err(error) if is_passage_text_difference(&error) => {
                    if attempt + 1 == attempts {
                        return Err(ChatCompletionsError::FreshAnnotationRetryLimit {
                            limit: self.config.max_fresh_annotation_retries,
                            source: Box::new(error),
                        });
                    }
                    eprintln!(
                        "Restored transcript window {} failed text-integrity validation; starting clean retry {}/{} (overall attempt {}/{attempts})",
                        task.window_index(),
                        attempt + 1,
                        self.config.max_fresh_annotation_retries,
                        attempt + 2,
                    );
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the bounded fresh-attempt loop always returns")
    }

    async fn restore_window(
        &self,
        task: &TranscriptRestorationTask<'_>,
    ) -> Result<TranscriptWindowRestorationResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let (restoration, diagnostics) = self
            .run_structured(
                StructuredTask {
                    instructions: message.instructions,
                    input: message.input,
                    schema: restoration_schema(),
                    workflow: ModelWorkflow::Restoration,
                    work_item_index: task.window_index,
                    max_repairs: self.config.max_final_answer_repairs,
                },
                |content| {
                    parse_restoration(content).and_then(|(restoration, fence)| {
                        validate_window_restoration(task, &restoration)
                            .map_err(ChatCompletionsError::InvalidWindowRestoration)?;
                        Ok((restoration, fence))
                    })
                },
                |error| format!("上一份恢复 JSON 无效：{error}。严格覆盖 owned_region 且不要认领 context，只返回完整 JSON。"),
            )
            .await?;
        Ok(TranscriptWindowRestorationResult {
            restoration,
            diagnostics: diagnostics.restoration(),
        })
    }

    async fn compare_passages(
        &self,
        task: &ComparativeRankingTask,
    ) -> Result<ComparativeRankingBatchResult, ChatCompletionsError> {
        let message = task
            .message()
            .map_err(ChatCompletionsError::SerializeTask)?;
        let workflow = match task.metric() {
            ComparativeMetric::Importance => ModelWorkflow::ImportanceComparison,
            ComparativeMetric::Novelty => ModelWorkflow::NoveltyComparison,
        };
        let (comparisons, diagnostics) = self
            .run_structured(
                StructuredTask {
                    instructions: message.instructions,
                    input: message.input,
                    schema: comparative_schema(),
                    workflow,
                    work_item_index: task.task_index(),
                    max_repairs: self.config.max_final_answer_repairs,
                },
                |content| {
                    let (proposed, fence) = parse_json::<ProposedComparativeRanking>(content)
                        .map_err(ChatCompletionsError::InvalidComparativeRankingJson)?;
                    let decisions = task
                        .validate(proposed)
                        .map_err(ChatCompletionsError::InvalidComparativeRanking)?;
                    Ok((decisions, fence))
                },
                |error| format!("上一份比较 JSON 无效：{error}。每组只能选择该组的 A/B/C/D，只返回完整 JSON。"),
            )
            .await?;
        Ok(ComparativeRankingBatchResult {
            metric: task.metric(),
            batch_index: task.batch_index(),
            comparisons,
            diagnostics: diagnostics.annotation(),
        })
    }

    async fn classify_passage_boundaries(
        &self,
        task: &BoundaryBatchTask,
    ) -> Result<BoundaryBatchResult, ChatCompletionsError> {
        let input = serde_json::to_string(task).map_err(ChatCompletionsError::SerializeTask)?;
        let (decisions, diagnostics) = self
            .run_structured(
                StructuredTask {
                    instructions: PASSAGE_BOUNDARY_INSTRUCTIONS,
                    input,
                    schema: boundary_schema(),
                    workflow: ModelWorkflow::PassageBoundaries,
                    work_item_index: task.batch_index(),
                    max_repairs: self.config.max_final_answer_repairs,
                },
                |content| {
                    let (proposed, fence) = parse_json(content)
                        .map_err(ChatCompletionsError::InvalidBoundaryJson)?;
                    task.validate(&proposed)
                        .map_err(ChatCompletionsError::InvalidBoundaries)?;
                    Ok((proposed, fence))
                },
                |error| format!("上一份切分代价 JSON 无效：{error}。完整回答所有 owned boundary，每项 cut_cost 为 0 到 5；只返回指定 JSON。"),
            )
            .await?;
        Ok(BoundaryBatchResult {
            batch_index: task.batch_index(),
            decisions,
            diagnostics: diagnostics.annotation(),
        })
    }
}

#[derive(Default)]
struct CodexDiagnostics {
    final_answer_repairs: usize,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    accepted_json_fence: bool,
}

impl CodexDiagnostics {
    fn annotation(self) -> AnnotationDiagnostics {
        AnnotationDiagnostics {
            provider_retries: 0,
            tool_rounds: 0,
            final_answer_repairs: self.final_answer_repairs,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            accepted_json_fence: self.accepted_json_fence,
        }
    }

    fn restoration(self) -> RestorationDiagnostics {
        RestorationDiagnostics {
            provider_retries: 0,
            final_answer_repairs: self.final_answer_repairs,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            accepted_json_fence: self.accepted_json_fence,
        }
    }
}

struct AppServer {
    writer: Mutex<ChildStdin>,
    _child: Mutex<Child>,
    pending: StdMutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    subscribers: StdMutex<HashMap<String, mpsc::UnboundedSender<Result<Value, String>>>>,
    next_id: AtomicU64,
    _workspace: TempDir,
}

impl AppServer {
    async fn launch(executable: &Path) -> Result<Arc<Self>, String> {
        let workspace = tempfile::tempdir().map_err(|error| error.to_string())?;
        let mut child = Command::new(executable)
            .args([
                "--disable",
                "shell_tool",
                "--disable",
                "unified_exec",
                "--disable",
                "apps",
                "--disable",
                "browser_use",
                "--disable",
                "computer_use",
                "--disable",
                "image_generation",
                "--disable",
                "multi_agent",
                "--disable",
                "plugins",
                "--disable",
                "skill_search",
                "--disable",
                "view_image",
                "--config",
                "web_search=\"disabled\"",
                "app-server",
                "--listen",
                "stdio://",
            ])
            .current_dir(workspace.path())
            .env_remove("BEYOND_SLIDES_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", executable.display()))?;
        let stdin = child.stdin.take().ok_or("Codex app-server has no stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Codex app-server has no stdout")?;
        let server = Arc::new(Self {
            writer: Mutex::new(stdin),
            _child: Mutex::new(child),
            pending: StdMutex::new(HashMap::new()),
            subscribers: StdMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            _workspace: workspace,
        });
        tokio::spawn(read_messages(Arc::downgrade(&server), stdout));
        server
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "beyond_slides",
                        "title": "BeyondSlides",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;
        server.notify("initialized", json!({})).await?;
        Ok(server)
    }

    async fn start_thread(&self, model: &str) -> Result<String, String> {
        let result = self
            .request(
                "thread/start",
                json!({
                    "model": model,
                    "cwd": self._workspace.path(),
                    "approvalPolicy": "never",
                    "sandbox": "read-only",
                    "ephemeral": true,
                    "baseInstructions": "Do not use shell, filesystem, web, MCP, skills, subagents, or any other tools. Transform only the text in the user request and return the required structured JSON.",
                    "serviceName": "beyond_slides",
                }),
            )
            .await?;
        result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "thread/start returned no thread id".into())
    }

    fn subscribe(
        &self,
        thread_id: &str,
    ) -> Result<mpsc::UnboundedReceiver<Result<Value, String>>, String> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let old = self
            .subscribers
            .lock()
            .map_err(|_| "Codex subscriber registry is unavailable")?
            .insert(thread_id.to_owned(), sender);
        if old.is_some() {
            return Err(format!("thread {thread_id} already has a subscriber"));
        }
        Ok(receiver)
    }

    async fn turn(
        &self,
        thread_id: &str,
        params: Value,
        events: &mut mpsc::UnboundedReceiver<Result<Value, String>>,
        timeout: Duration,
    ) -> Result<CodexTurn, String> {
        let result = self.request("turn/start", params).await?;
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or("turn/start returned no turn id")?
            .to_owned();
        let wait = async {
            let mut content = None;
            let mut input_tokens = None;
            let mut output_tokens = None;
            while let Some(event) = events.recv().await {
                let event = event?;
                let method = event.get("method").and_then(Value::as_str).unwrap_or("");
                let params = &event["params"];
                match method {
                    "thread/tokenUsage/updated" if params["turnId"].as_str() == Some(&turn_id) => {
                        input_tokens =
                            nonnegative_u64(params.pointer("/tokenUsage/last/inputTokens"));
                        output_tokens =
                            nonnegative_u64(params.pointer("/tokenUsage/last/outputTokens"));
                    }
                    "item/started" | "item/completed"
                        if params["turnId"].as_str() == Some(&turn_id) =>
                    {
                        let item = &params["item"];
                        match item["type"].as_str() {
                            Some("agentMessage")
                                if method == "item/completed"
                                    && item["phase"].as_str() != Some("commentary") =>
                            {
                                content = item["text"].as_str().map(str::to_owned);
                            }
                            Some(
                                "commandExecution" | "fileChange" | "mcpToolCall"
                                | "dynamicToolCall" | "collabToolCall" | "webSearch",
                            ) => {
                                return Err(format!(
                                    "Codex attempted forbidden tool item {}",
                                    item["type"]
                                ));
                            }
                            _ => {}
                        }
                    }
                    "turn/completed" if params["turn"]["id"].as_str() == Some(&turn_id) => {
                        let status = params["turn"]["status"].as_str().unwrap_or("unknown");
                        if status != "completed" {
                            return Err(params["turn"]["error"]["message"]
                                .as_str()
                                .unwrap_or("Codex turn did not complete")
                                .to_owned());
                        }
                        return Ok(CodexTurn {
                            content: content
                                .ok_or("Codex completed without a final agent message")?,
                            input_tokens,
                            output_tokens,
                        });
                    }
                    _ => {}
                }
            }
            Err("Codex app-server closed the event stream".into())
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| format!("Codex turn timed out after {} seconds", timeout.as_secs()))?
            .map_err(|error| format!("thread {thread_id}: {error}"))
    }

    async fn unsubscribe(&self, thread_id: &str) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.remove(thread_id);
        }
        let _ = self
            .request("thread/unsubscribe", json!({"threadId": thread_id}))
            .await;
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| "Codex request registry is unavailable")?
            .insert(id, sender);
        if let Err(error) = self
            .send(json!({"method": method, "id": id, "params": params}))
            .await
        {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error);
        }
        let response = tokio::time::timeout(RPC_TIMEOUT, receiver)
            .await
            .map_err(|_| format!("Codex RPC {method} timed out"))?
            .map_err(|_| format!("Codex RPC {method} response channel closed"))??;
        if let Some(error) = response.get("error") {
            return Err(format!("Codex RPC {method} failed: {error}"));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| format!("Codex RPC {method} returned neither result nor error"))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"method": method, "params": params})).await
    }

    async fn send(&self, message: Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        line.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&line)
            .await
            .map_err(|error| format!("could not write to Codex app-server: {error}"))?;
        writer.flush().await.map_err(|error| error.to_string())
    }
}

struct CodexTurn {
    content: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

async fn read_messages(server: Weak<AppServer>, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            fail_server(&server, format!("Codex emitted invalid JSON: {line}"));
            return;
        };
        let Some(server) = server.upgrade() else {
            return;
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            if let Ok(mut pending) = server.pending.lock()
                && let Some(sender) = pending.remove(&id)
            {
                let _ = sender.send(Ok(message));
            }
            continue;
        }
        let thread_id = message
            .pointer("/params/threadId")
            .or_else(|| message.pointer("/params/thread/id"))
            .and_then(Value::as_str);
        if let Some(thread_id) = thread_id
            && let Ok(subscribers) = server.subscribers.lock()
            && let Some(sender) = subscribers.get(thread_id)
        {
            let _ = sender.send(Ok(message));
        }
    }
    fail_server(&server, "Codex app-server exited".into());
}

fn fail_server(server: &Weak<AppServer>, error: String) {
    let Some(server) = server.upgrade() else {
        return;
    };
    if let Ok(mut pending) = server.pending.lock() {
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(error.clone()));
        }
    }
    if let Ok(subscribers) = server.subscribers.lock() {
        for sender in subscribers.values() {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

fn nonnegative_u64(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_i64)
        .and_then(|value| value.try_into().ok())
}

fn add_optional(total: Option<u64>, value: Option<u64>) -> Option<u64> {
    match (total, value) {
        (Some(total), Some(value)) => total.checked_add(value),
        _ => None,
    }
}

fn codex_error(error: impl fmt::Display) -> ChatCompletionsError {
    ChatCompletionsError::Provider(format!("Codex app-server: {error}"))
}

fn parse_json<T: serde::de::DeserializeOwned>(
    content: &str,
) -> Result<(T, bool), serde_json::Error> {
    serde_json::from_str(content).map(|value| (value, false))
}

fn annotation_schema() -> Value {
    json!({"type":"object","properties":{"passages":{"type":"array","items":{"type":"object","properties":{"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":0},"importance":{"type":"integer","minimum":0,"maximum":5},"novelty":{"type":"integer","minimum":0,"maximum":5},"related_slides":{"type":"array","items":{"type":"integer","minimum":0}},"summary":{"type":["string","null"]},"comparison_note":{"type":["string","null"]}},"required":["start","end","importance","novelty","related_slides","summary","comparison_note"],"additionalProperties":false}}},"required":["passages"],"additionalProperties":false})
}

fn restored_annotation_schema() -> Value {
    json!({"type":"object","properties":{"passages":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"connection_strength":{"type":"integer","minimum":0,"maximum":5},"related_slides":{"type":"array","items":{"type":"integer","minimum":0}},"summary":{"type":["string","null"]},"comparison_note":{"type":["string","null"]}},"required":["text","connection_strength","related_slides","summary","comparison_note"],"additionalProperties":false}}},"required":["passages"],"additionalProperties":false})
}

fn restoration_schema() -> Value {
    // Strict structured output does not accept `oneOf`. `text` is therefore a
    // required wire field for both variants; Serde ignores its empty value when
    // decoding an `omitted_disfluency` into the narrower domain enum.
    json!({"type":"object","properties":{"spans":{"type":"array","items":{"type":"object","properties":{"kind":{"enum":["text","omitted_disfluency"]},"source_start":{"type":"integer","minimum":0},"source_end":{"type":"integer","minimum":0},"text":{"type":"string"}},"required":["kind","source_start","source_end","text"],"additionalProperties":false}}},"required":["spans"],"additionalProperties":false})
}

fn comparative_schema() -> Value {
    json!({"type":"object","properties":{"comparisons":{"type":"array","items":{"type":"object","properties":{"comparison_id":{"type":"integer","minimum":0},"most":{"enum":["A","B","C","D"]},"least":{"enum":["A","B","C","D"]}},"required":["comparison_id","most","least"],"additionalProperties":false}}},"required":["comparisons"],"additionalProperties":false})
}

fn boundary_schema() -> Value {
    json!({"type":"object","properties":{"windows":{"type":"array","items":{"type":"object","properties":{"window_index":{"type":"integer","minimum":0},"boundaries":{"type":"array","items":{"type":"object","properties":{"after_atom":{"type":"integer","minimum":0},"cut_cost":{"type":"integer","minimum":0,"maximum":5}},"required":["after_atom","cut_cost"],"additionalProperties":false}}},"required":["window_index","boundaries"],"additionalProperties":false}}},"required":["windows"],"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_forbid_unknown_top_level_fields() {
        for schema in [
            annotation_schema(),
            restored_annotation_schema(),
            restoration_schema(),
            comparative_schema(),
            boundary_schema(),
        ] {
            assert_eq!(schema["additionalProperties"], false);
        }
    }

    #[test]
    fn token_usage_rejects_negative_and_non_integer_values() {
        assert_eq!(nonnegative_u64(Some(&json!(42))), Some(42));
        assert_eq!(nonnegative_u64(Some(&json!(-1))), None);
        assert_eq!(nonnegative_u64(Some(&json!("42"))), None);
    }

    #[test]
    fn omitted_disfluency_accepts_the_flat_schema_padding_field() {
        let (restoration, _) = parse_restoration(
            r#"{"spans":[{"kind":"omitted_disfluency","source_start":0,"source_end":0,"text":""}]}"#,
        )
        .expect("Serde ignores the transport-only text field");
        assert!(matches!(
            restoration.spans.as_slice(),
            [crate::RestoredTranscriptSpan::OmittedDisfluency { .. }]
        ));
    }
}
