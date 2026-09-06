use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MODEL_TRACE_FORMAT_VERSION: u32 = 1;

/// An append-only, crash-tolerant record of model exchanges.
///
/// The trace contains lecture text and model output, but never provider
/// credentials or authorization headers. Clones share one synchronized writer.
#[derive(Clone)]
pub struct ModelExchangeTrace {
    inner: Arc<TraceInner>,
}

struct TraceInner {
    path: PathBuf,
    state: Mutex<TraceState>,
}

struct TraceState {
    file: File,
    next_event_index: u64,
    next_exchange_id: u64,
}

impl ModelExchangeTrace {
    /// Opens a trace for append. Existing complete records determine the next
    /// identifiers, so resumed runs cannot reuse an exchange ID.
    ///
    /// A malformed final partial line is discarded before new records are
    /// appended. Corruption in any newline-terminated record is rejected.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let path = path.into();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let ends_with_newline = bytes.last().is_none_or(|byte| *byte == b'\n');
        let mut next_event_index = 0;
        let mut next_exchange_id = 0;
        let mut partial_tail_start = None;
        let line_count = bytes.split(|byte| *byte == b'\n').count();
        for (line_index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let is_partial_tail = !ends_with_newline && line_index + 1 == line_count;
            let record = match serde_json::from_slice::<ModelTraceRecord>(line) {
                Ok(record) => record,
                Err(_) if is_partial_tail => {
                    partial_tail_start = Some(bytes.len() - line.len());
                    continue;
                }
                Err(error) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "invalid model trace record {} in {}: {error}",
                            line_index + 1,
                            path.display()
                        ),
                    ));
                }
            };
            next_event_index = next_event_index.max(record.event_index.saturating_add(1));
            next_exchange_id = next_exchange_id.max(record.exchange_id.saturating_add(1));
        }

        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        if let Some(partial_tail_start) = partial_tail_start {
            file.set_len(partial_tail_start as u64)?;
        } else if !bytes.is_empty() && !ends_with_newline {
            file.write_all(b"\n")?;
            file.flush()?;
        }

        Ok(Self {
            inner: Arc::new(TraceInner {
                path,
                state: Mutex::new(TraceState {
                    file,
                    next_event_index,
                    next_exchange_id,
                }),
            }),
        })
    }

    pub(crate) fn record_request(
        &self,
        context: ModelTraceContext,
        provider_attempt: usize,
        endpoint: &str,
        model: &str,
        request: &impl Serialize,
        options: &impl Serialize,
    ) -> Result<u64, io::Error> {
        let request = to_value(request, "model request")?;
        let options = to_value(options, "model request options")?;
        let mut state = self.lock()?;
        let exchange_id = state.next_exchange_id;
        state.next_exchange_id = state.next_exchange_id.saturating_add(1);
        self.append_locked(
            &mut state,
            exchange_id,
            context,
            ModelTraceEvent::Request {
                provider_attempt,
                endpoint: endpoint.into(),
                model: model.into(),
                request,
                options,
            },
        )?;
        Ok(exchange_id)
    }

    pub(crate) fn record_response(
        &self,
        exchange_id: u64,
        context: ModelTraceContext,
        provider_attempt: usize,
        elapsed: Duration,
        response: &impl Serialize,
        raw_response: Option<Value>,
    ) -> Result<(), io::Error> {
        self.append(
            exchange_id,
            context,
            ModelTraceEvent::Response {
                provider_attempt,
                elapsed_ms: duration_milliseconds(elapsed),
                response: to_value(response, "normalized model response")?,
                raw_response,
            },
        )
    }

    pub(crate) fn record_provider_error(
        &self,
        exchange_id: u64,
        context: ModelTraceContext,
        failure: ModelProviderFailure,
    ) -> Result<(), io::Error> {
        self.append(
            exchange_id,
            context,
            ModelTraceEvent::ProviderError {
                provider_attempt: failure.provider_attempt,
                elapsed_ms: duration_milliseconds(failure.elapsed),
                retryable: failure.retryable,
                will_retry: failure.will_retry,
                error: failure.error,
            },
        )
    }

    pub(crate) fn record_validation(
        &self,
        exchange_id: u64,
        context: ModelTraceContext,
        accepted: bool,
        category: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), io::Error> {
        self.append(
            exchange_id,
            context,
            ModelTraceEvent::Validation {
                accepted,
                category: category.map(Into::into),
                error: error.map(Into::into),
            },
        )
    }

    pub(crate) fn record_processing_error(
        &self,
        exchange_id: u64,
        context: ModelTraceContext,
        category: &str,
        error: &str,
    ) -> Result<(), io::Error> {
        self.append(
            exchange_id,
            context,
            ModelTraceEvent::ProcessingError {
                category: category.into(),
                error: error.into(),
            },
        )
    }

    fn append(
        &self,
        exchange_id: u64,
        context: ModelTraceContext,
        event: ModelTraceEvent,
    ) -> Result<(), io::Error> {
        let mut state = self.lock()?;
        self.append_locked(&mut state, exchange_id, context, event)
    }

    fn append_locked(
        &self,
        state: &mut TraceState,
        exchange_id: u64,
        context: ModelTraceContext,
        event: ModelTraceEvent,
    ) -> Result<(), io::Error> {
        let record = ModelTraceRecord {
            format_version: MODEL_TRACE_FORMAT_VERSION,
            event_index: state.next_event_index,
            timestamp_unix_ms: timestamp_milliseconds()?,
            exchange_id,
            workflow: context.workflow,
            window_index: context.work_item_index,
            conversation_turn: context.conversation_turn,
            request_kind: context.request_kind,
            event,
        };
        let mut line = serde_json::to_vec(&record).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not serialize model trace record: {error}"),
            )
        })?;
        line.push(b'\n');
        state.file.write_all(&line).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not append model trace {}: {error}",
                    self.inner.path.display()
                ),
            )
        })?;
        state.file.flush().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not flush model trace {}: {error}",
                    self.inner.path.display()
                ),
            )
        })?;
        state.next_event_index = state.next_event_index.saturating_add(1);
        Ok(())
    }

    fn lock(&self) -> Result<MutexGuard<'_, TraceState>, io::Error> {
        self.inner.state.lock().map_err(|_| {
            io::Error::other(format!(
                "model trace writer for {} is unavailable after a panic",
                self.inner.path.display()
            ))
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModelTraceContext {
    pub workflow: ModelWorkflow,
    pub work_item_index: usize,
    pub conversation_turn: usize,
    pub request_kind: ModelRequestKind,
}

pub(crate) struct ModelProviderFailure {
    pub provider_attempt: usize,
    pub elapsed: Duration,
    pub retryable: bool,
    pub will_retry: bool,
    pub error: ModelProviderError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelWorkflow {
    Annotation,
    Restoration,
    ImportanceComparison,
    NoveltyComparison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestKind {
    Initial,
    ToolFollowUp,
    Repair,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelTraceRecord {
    pub format_version: u32,
    pub event_index: u64,
    pub timestamp_unix_ms: u64,
    pub exchange_id: u64,
    pub workflow: ModelWorkflow,
    pub window_index: usize,
    pub conversation_turn: usize,
    pub request_kind: ModelRequestKind,
    #[serde(flatten)]
    pub event: ModelTraceEvent,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ModelTraceEvent {
    Request {
        provider_attempt: usize,
        endpoint: String,
        model: String,
        request: Value,
        options: Value,
    },
    Response {
        provider_attempt: usize,
        elapsed_ms: u64,
        response: Value,
        raw_response: Option<Value>,
    },
    ProviderError {
        provider_attempt: usize,
        elapsed_ms: u64,
        retryable: bool,
        will_retry: bool,
        error: ModelProviderError,
    },
    Validation {
        accepted: bool,
        category: Option<String>,
        error: Option<String>,
    },
    ProcessingError {
        category: String,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelProviderError {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

pub fn read_model_trace(path: &Path) -> Result<Vec<ModelTraceRecord>, io::Error> {
    let bytes = fs::read(path)?;
    bytes
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(line_index, line)| {
            serde_json::from_slice(line).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "invalid model trace record {} in {}: {error}",
                        line_index + 1,
                        path.display()
                    ),
                )
            })
        })
        .collect()
}

fn to_value(value: &impl Serialize, kind: &str) -> Result<Value, io::Error> {
    serde_json::to_value(value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not serialize {kind} for the model trace: {error}"),
        )
    })
}

fn timestamp_milliseconds() -> Result<u64, io::Error> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            io::Error::other(format!("system clock is before the Unix epoch: {error}"))
        })?;
    Ok(duration_milliseconds(elapsed))
}

fn duration_milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread};

    use serde_json::json;

    use super::*;

    fn context(work_item_index: usize) -> ModelTraceContext {
        ModelTraceContext {
            workflow: ModelWorkflow::Restoration,
            work_item_index,
            conversation_turn: 0,
            request_kind: ModelRequestKind::Initial,
        }
    }

    #[test]
    fn resumed_trace_continues_identifiers() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("model-trace.jsonl");
        let trace = ModelExchangeTrace::open(&path)?;
        let first = trace.record_request(
            context(0),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);

        let trace = ModelExchangeTrace::open(&path)?;
        let second = trace.record_request(
            context(1),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;

        assert_eq!((first, second), (0, 1));
        let records = read_model_trace(&path)?;
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].event_index, 1);
        Ok(())
    }

    #[test]
    fn concurrent_writers_emit_complete_json_lines() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("model-trace.jsonl");
        let trace = Arc::new(ModelExchangeTrace::open(&path)?);
        let threads: Vec<_> = (0..8)
            .map(|window_index| {
                let trace = Arc::clone(&trace);
                thread::spawn(move || {
                    trace.record_request(
                        context(window_index),
                        0,
                        "https://example.test/v1",
                        "model",
                        &json!({ "window": window_index }),
                        &json!({}),
                    )
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("trace thread does not panic")?;
        }

        let records = read_model_trace(&path)?;
        assert_eq!(records.len(), 8);
        assert_eq!(
            records
                .iter()
                .map(|record| record.exchange_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            8
        );
        Ok(())
    }

    #[test]
    fn resume_preserves_a_complete_record_without_a_final_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("model-trace.jsonl");
        let trace = ModelExchangeTrace::open(&path)?;
        trace.record_request(
            context(0),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);

        let mut contents = fs::read(&path)?;
        assert_eq!(contents.pop(), Some(b'\n'));
        fs::write(&path, contents)?;

        let trace = ModelExchangeTrace::open(&path)?;
        let second = trace.record_request(
            context(1),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);

        assert_eq!(second, 1);
        let records = read_model_trace(&path)?;
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].event_index, 1);
        Ok(())
    }

    #[test]
    fn resume_discards_a_partial_crash_record_and_remains_reopenable()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("model-trace.jsonl");
        let trace = ModelExchangeTrace::open(&path)?;
        trace.record_request(
            context(0),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(br#"{"incomplete":"#)?;

        let trace = ModelExchangeTrace::open(&path)?;
        let second = trace.record_request(
            context(1),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);

        assert_eq!(second, 1);
        assert_eq!(read_model_trace(&path)?.len(), 2);

        let trace = ModelExchangeTrace::open(&path)?;
        let third = trace.record_request(
            context(2),
            0,
            "https://example.test/v1",
            "model",
            &json!({ "messages": [] }),
            &json!({}),
        )?;
        drop(trace);

        assert_eq!(third, 2);
        let records = read_model_trace(&path)?;
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].event_index, 2);
        Ok(())
    }
}
