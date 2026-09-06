use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::File,
    io::{self, BufRead, BufReader},
    path::Path,
};

use crate::{
    MODEL_TRACE_FORMAT_VERSION, ModelRequestKind, ModelTraceEvent, ModelTraceRecord, ModelWorkflow,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTraceSummary {
    pub total_lines: usize,
    pub parsed_events: usize,
    pub request_count: usize,
    pub response_count: usize,
    pub provider_error_count: usize,
    pub scheduled_retry_count: usize,
    pub incomplete_exchange_count: usize,
    pub accepted_validation_count: usize,
    pub rejected_validation_count: usize,
    pub repair_turn_count: usize,
    pub affected_window_count: usize,
    pub malformed_lines: Vec<MalformedTraceLine>,
    pub failure_categories: BTreeMap<String, usize>,
    pub workflows: BTreeMap<String, WorkflowTraceSummary>,
    pub affected_windows: Vec<AffectedTraceWindow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowTraceSummary {
    pub request_count: usize,
    pub response_count: usize,
    pub provider_error_count: usize,
    pub validation_failure_count: usize,
    pub processing_error_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedTraceWindow {
    pub workflow: String,
    pub window_index: usize,
    pub failures: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedTraceLine {
    pub line_number: usize,
    pub error: String,
}

pub fn summarize_model_trace(path: &Path) -> Result<ModelTraceSummary, io::Error> {
    let file = File::open(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not open model trace {}: {error}", path.display()),
        )
    })?;
    let mut accumulator = SummaryAccumulator::default();
    let mut line = Vec::new();
    let mut reader = BufReader::new(file);
    loop {
        line.clear();
        let bytes_read = reader.read_until(b'\n', &mut line)?;
        if bytes_read == 0 {
            break;
        }
        accumulator.total_lines += 1;
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<ModelTraceRecord>(&line) {
            Ok(record) => accumulator.record(record),
            Err(error) => {
                accumulator.increment_failure("trace:malformed_line", None);
                accumulator.malformed_lines.push(MalformedTraceLine {
                    line_number: accumulator.total_lines,
                    error: error.to_string(),
                });
            }
        }
    }
    Ok(accumulator.finish())
}

pub fn render_model_trace_summary(summary: &ModelTraceSummary) -> String {
    let mut output = String::new();
    output.push_str("Model exchange trace summary\n");
    output.push_str(&format!(
        "Events: {} parsed from {} lines; {} malformed lines\n",
        summary.parsed_events,
        summary.total_lines,
        summary.malformed_lines.len()
    ));
    output.push_str(&format!(
        "Provider attempts: {} requests, {} responses, {} errors, {} scheduled retries, {} incomplete exchanges\n",
        summary.request_count,
        summary.response_count,
        summary.provider_error_count,
        summary.scheduled_retry_count,
        summary.incomplete_exchange_count
    ));
    output.push_str(&format!(
        "Structured responses: {} accepted, {} rejected; {} repair turns; {} affected windows\n",
        summary.accepted_validation_count,
        summary.rejected_validation_count,
        summary.repair_turn_count,
        summary.affected_window_count
    ));

    if !summary.workflows.is_empty() {
        output.push_str("\nBy workflow:\n");
        for (workflow, counts) in &summary.workflows {
            output.push_str(&format!(
                "  {workflow}: {} requests, {} responses, {} provider errors, {} validation failures, {} processing errors\n",
                counts.request_count,
                counts.response_count,
                counts.provider_error_count,
                counts.validation_failure_count,
                counts.processing_error_count
            ));
        }
    }

    if !summary.failure_categories.is_empty() {
        output.push_str("\nFailure categories:\n");
        for (category, count) in &summary.failure_categories {
            output.push_str(&format!("  {category}: {count}\n"));
        }
    }

    if !summary.affected_windows.is_empty() {
        output.push_str("\nAffected windows (zero-based):\n");
        for window in &summary.affected_windows {
            let failures = window
                .failures
                .iter()
                .map(|(category, count)| format!("{category} ×{count}"))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "  {} window {}: {failures}\n",
                window.workflow, window.window_index
            ));
        }
    }

    if !summary.malformed_lines.is_empty() {
        output.push_str("\nMalformed trace lines:\n");
        for malformed in &summary.malformed_lines {
            output.push_str(&format!(
                "  line {}: {}\n",
                malformed.line_number, malformed.error
            ));
        }
    }
    output
}

#[derive(Default)]
struct SummaryAccumulator {
    total_lines: usize,
    parsed_events: usize,
    request_count: usize,
    response_count: usize,
    provider_error_count: usize,
    scheduled_retry_count: usize,
    accepted_validation_count: usize,
    rejected_validation_count: usize,
    request_exchanges: HashSet<u64>,
    completed_exchanges: HashSet<u64>,
    repair_turns: BTreeSet<(String, usize, usize)>,
    malformed_lines: Vec<MalformedTraceLine>,
    failure_categories: BTreeMap<String, usize>,
    workflows: BTreeMap<String, WorkflowTraceSummary>,
    window_failures: BTreeMap<(String, usize), BTreeMap<String, usize>>,
}

impl SummaryAccumulator {
    fn record(&mut self, record: ModelTraceRecord) {
        self.parsed_events += 1;
        let workflow = workflow_name(record.workflow).to_owned();
        let window = (workflow.clone(), record.window_index);
        if record.format_version != MODEL_TRACE_FORMAT_VERSION {
            self.increment_failure("trace:unsupported_format_version", Some(window.clone()));
        }
        let workflow_counts = self.workflows.entry(workflow.clone()).or_default();
        match record.event {
            ModelTraceEvent::Request { .. } => {
                self.request_count += 1;
                workflow_counts.request_count += 1;
                self.request_exchanges.insert(record.exchange_id);
                if record.request_kind == ModelRequestKind::Repair {
                    self.repair_turns.insert((
                        workflow,
                        record.window_index,
                        record.conversation_turn,
                    ));
                }
            }
            ModelTraceEvent::Response { .. } => {
                self.response_count += 1;
                workflow_counts.response_count += 1;
                self.completed_exchanges.insert(record.exchange_id);
            }
            ModelTraceEvent::ProviderError {
                will_retry, error, ..
            } => {
                self.provider_error_count += 1;
                workflow_counts.provider_error_count += 1;
                self.completed_exchanges.insert(record.exchange_id);
                if will_retry {
                    self.scheduled_retry_count += 1;
                }
                self.increment_failure(&format!("provider:{}", error.kind), Some(window));
            }
            ModelTraceEvent::Validation {
                accepted, category, ..
            } => {
                if accepted {
                    self.accepted_validation_count += 1;
                } else {
                    self.rejected_validation_count += 1;
                    workflow_counts.validation_failure_count += 1;
                    self.increment_failure(
                        category.as_deref().unwrap_or("validation"),
                        Some(window),
                    );
                }
            }
            ModelTraceEvent::ProcessingError { category, .. } => {
                workflow_counts.processing_error_count += 1;
                self.increment_failure(&category, Some(window));
            }
        }
    }

    fn increment_failure(&mut self, category: &str, window: Option<(String, usize)>) {
        *self
            .failure_categories
            .entry(category.to_owned())
            .or_default() += 1;
        if let Some(window) = window {
            *self
                .window_failures
                .entry(window)
                .or_default()
                .entry(category.to_owned())
                .or_default() += 1;
        }
    }

    fn finish(mut self) -> ModelTraceSummary {
        let incomplete_exchange_count = self
            .request_exchanges
            .difference(&self.completed_exchanges)
            .count();
        if incomplete_exchange_count > 0 {
            self.failure_categories.insert(
                "trace:incomplete_exchange".into(),
                incomplete_exchange_count,
            );
        }
        let affected_windows = self
            .window_failures
            .into_iter()
            .map(|((workflow, window_index), failures)| AffectedTraceWindow {
                workflow,
                window_index,
                failures,
            })
            .collect::<Vec<_>>();
        ModelTraceSummary {
            total_lines: self.total_lines,
            parsed_events: self.parsed_events,
            request_count: self.request_count,
            response_count: self.response_count,
            provider_error_count: self.provider_error_count,
            scheduled_retry_count: self.scheduled_retry_count,
            incomplete_exchange_count,
            accepted_validation_count: self.accepted_validation_count,
            rejected_validation_count: self.rejected_validation_count,
            repair_turn_count: self.repair_turns.len(),
            affected_window_count: affected_windows.len(),
            malformed_lines: self.malformed_lines,
            failure_categories: self.failure_categories,
            workflows: self.workflows,
            affected_windows,
        }
    }
}

fn workflow_name(workflow: ModelWorkflow) -> &'static str {
    match workflow {
        ModelWorkflow::Annotation => "annotation",
        ModelWorkflow::Restoration => "restoration",
        ModelWorkflow::ImportanceComparison => "importance_comparison",
        ModelWorkflow::NoveltyComparison => "novelty_comparison",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::ModelProviderError;

    fn record(exchange_id: u64, event_index: u64, event: ModelTraceEvent) -> ModelTraceRecord {
        ModelTraceRecord {
            format_version: MODEL_TRACE_FORMAT_VERSION,
            event_index,
            timestamp_unix_ms: 1,
            exchange_id,
            workflow: ModelWorkflow::Restoration,
            window_index: 7,
            conversation_turn: usize::try_from(exchange_id).expect("small test ID"),
            request_kind: if exchange_id == 0 {
                ModelRequestKind::Initial
            } else {
                ModelRequestKind::Repair
            },
            event,
        }
    }

    #[test]
    fn summary_classifies_failures_and_reports_incomplete_exchanges()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("trace.jsonl");
        let records = [
            record(
                0,
                0,
                ModelTraceEvent::Request {
                    provider_attempt: 0,
                    endpoint: "https://example.test/v1".into(),
                    model: "model".into(),
                    request: serde_json::json!({}),
                    options: serde_json::json!({}),
                },
            ),
            record(
                0,
                1,
                ModelTraceEvent::ProviderError {
                    provider_attempt: 0,
                    elapsed_ms: 1,
                    retryable: true,
                    will_retry: true,
                    error: ModelProviderError {
                        kind: "http_status".into(),
                        message: "unavailable".into(),
                        status: Some(503),
                        body: None,
                    },
                },
            ),
            record(
                1,
                2,
                ModelTraceEvent::Request {
                    provider_attempt: 1,
                    endpoint: "https://example.test/v1".into(),
                    model: "model".into(),
                    request: serde_json::json!({}),
                    options: serde_json::json!({}),
                },
            ),
            record(
                1,
                3,
                ModelTraceEvent::Validation {
                    accepted: false,
                    category: Some("outside_owned_region".into()),
                    error: Some("claimed context".into()),
                },
            ),
        ];
        let mut contents = records
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        contents.push_str("\n{broken\n");
        fs::write(&path, contents)?;

        let summary = summarize_model_trace(&path)?;

        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.provider_error_count, 1);
        assert_eq!(summary.scheduled_retry_count, 1);
        assert_eq!(summary.rejected_validation_count, 1);
        assert_eq!(summary.repair_turn_count, 1);
        assert_eq!(summary.incomplete_exchange_count, 1);
        assert_eq!(summary.malformed_lines.len(), 1);
        assert_eq!(summary.failure_categories["provider:http_status"], 1);
        assert_eq!(summary.failure_categories["outside_owned_region"], 1);
        assert_eq!(summary.failure_categories["trace:malformed_line"], 1);
        assert_eq!(summary.failure_categories["trace:incomplete_exchange"], 1);
        assert_eq!(summary.affected_window_count, 1);
        Ok(())
    }
}
