use std::{
    collections::HashSet,
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

use beyond_slides::{ModelTraceEvent, ModelTraceRecord};
use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize)]
pub(super) struct Usage {
    pub requests: u64,
    pub responses: u64,
    pub provider_errors: u64,
    pub retries: u64,
    pub hedges: u64,
    pub active_requests: usize,
    pub known_input_tokens: u64,
    pub known_cached_input_tokens: u64,
    pub known_output_tokens: u64,
    pub missing_input_usage: u64,
    pub missing_cached_input_usage: u64,
    pub missing_output_usage: u64,
}

#[derive(Default)]
pub(super) struct TraceCursor {
    offset: u64,
    active: HashSet<u64>,
    usage: Usage,
}

impl TraceCursor {
    /// Reads newly completed JSONL records only. A partial record is left for
    /// the next poll; this observer never repairs or modifies the trace.
    pub fn poll(
        &mut self,
        path: &Path,
        attempt_started_ms: u64,
        running: bool,
    ) -> Result<Usage, io::Error> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Usage::default()),
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() < self.offset {
            *self = Self::default();
        }
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;
        let mut line = Vec::new();
        loop {
            line.clear();
            let count = reader.read_until(b'\n', &mut line)?;
            if count == 0 || line.last() != Some(&b'\n') {
                break;
            }
            let record: ModelTraceRecord = serde_json::from_slice(&line)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            self.record(record, attempt_started_ms);
            self.offset += count as u64;
        }
        let mut usage = self.usage.clone();
        usage.active_requests = if running { self.active.len() } else { 0 };
        Ok(usage)
    }

    fn record(&mut self, record: ModelTraceRecord, attempt_started_ms: u64) {
        match record.event {
            ModelTraceEvent::Request {
                provider_attempt,
                hedge,
                ..
            } => {
                self.usage.requests += 1;
                self.usage.hedges += u64::from(hedge.is_some());
                self.usage.retries += u64::from(provider_attempt > 0 && hedge.is_none());
                if record.timestamp_unix_ms >= attempt_started_ms {
                    self.active.insert(record.exchange_id);
                }
            }
            ModelTraceEvent::Response {
                response,
                raw_response,
                ..
            } => {
                self.active.remove(&record.exchange_id);
                self.usage.responses += 1;
                match response["usage"]["prompt_tokens"].as_u64() {
                    Some(count) => {
                        self.usage.known_input_tokens =
                            self.usage.known_input_tokens.saturating_add(count)
                    }
                    None => self.usage.missing_input_usage += 1,
                }
                match cached_input_tokens(&response, raw_response.as_ref()) {
                    Some(count) => {
                        self.usage.known_cached_input_tokens =
                            self.usage.known_cached_input_tokens.saturating_add(count)
                    }
                    None => self.usage.missing_cached_input_usage += 1,
                }
                match response["usage"]["completion_tokens"].as_u64() {
                    Some(count) => {
                        self.usage.known_output_tokens =
                            self.usage.known_output_tokens.saturating_add(count)
                    }
                    None => self.usage.missing_output_usage += 1,
                }
            }
            ModelTraceEvent::ProviderError { .. } => {
                self.active.remove(&record.exchange_id);
                self.usage.provider_errors += 1;
            }
            ModelTraceEvent::Cancelled { .. } => {
                self.active.remove(&record.exchange_id);
            }
            _ => {}
        }
    }
}

impl Usage {
    pub fn add(&mut self, other: Self) {
        self.requests += other.requests;
        self.responses += other.responses;
        self.provider_errors += other.provider_errors;
        self.retries += other.retries;
        self.hedges += other.hedges;
        self.active_requests += other.active_requests;
        self.known_input_tokens += other.known_input_tokens;
        self.known_cached_input_tokens += other.known_cached_input_tokens;
        self.known_output_tokens += other.known_output_tokens;
        self.missing_input_usage += other.missing_input_usage;
        self.missing_cached_input_usage += other.missing_cached_input_usage;
        self.missing_output_usage += other.missing_output_usage;
    }
}

fn cached_input_tokens(
    response: &serde_json::Value,
    raw_response: Option<&serde_json::Value>,
) -> Option<u64> {
    response
        .pointer("/usage/prompt_tokens_details/cached_tokens")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            raw_response?
                .pointer("/usage/prompt_tokens_details/cached_tokens")?
                .as_u64()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn record(event: serde_json::Value, index: u64) -> Vec<u8> {
        let mut record = json!({
            "format_version": 1, "event_index": index, "timestamp_unix_ms": 100,
            "exchange_id": 0, "workflow": "restoration", "window_index": 0,
            "conversation_turn": 0, "request_kind": "initial"
        });
        record
            .as_object_mut()
            .unwrap()
            .extend(event.as_object().unwrap().clone());
        let mut bytes = serde_json::to_vec(&record).unwrap();
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn partial_responses_are_not_counted_twice_and_missing_usage_is_visible()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&record(json!({"event":"request", "provider_attempt":0, "endpoint":"test", "model":"test", "request":{}, "options":{}}), 0))?;
        let mut cursor = TraceCursor::default();
        assert_eq!(cursor.poll(file.path(), 90, true)?.active_requests, 1);
        let response = record(
            json!({"event":"response", "provider_attempt":0, "elapsed_ms":1, "response":{"usage":{"prompt_tokens":12}}, "raw_response":null}),
            1,
        );
        let split = response.len() / 2;
        file.write_all(&response[..split])?;
        assert_eq!(cursor.poll(file.path(), 90, true)?.responses, 0);
        file.write_all(&response[split..])?;
        let usage = cursor.poll(file.path(), 90, true)?;
        assert_eq!(usage.active_requests, 0);
        assert_eq!(usage.known_input_tokens, 12);
        assert_eq!(usage.missing_cached_input_usage, 1);
        assert_eq!(usage.missing_output_usage, 1);
        assert_eq!(cursor.poll(file.path(), 90, true)?.known_input_tokens, 12);
        Ok(())
    }

    #[test]
    fn cached_input_distinguishes_explicit_zero_from_missing_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&record(
            json!({
                "event":"response",
                "provider_attempt":0,
                "elapsed_ms":1,
                "response":{"usage":{"prompt_tokens":10,"completion_tokens":1}},
                "raw_response":{"usage":{"prompt_tokens_details":{"cached_tokens":0}}}
            }),
            0,
        ))?;
        let mut cursor = TraceCursor::default();
        let usage = cursor.poll(file.path(), 90, true)?;
        assert_eq!(usage.known_input_tokens, 10);
        assert_eq!(usage.known_cached_input_tokens, 0);
        assert_eq!(usage.missing_cached_input_usage, 0);

        file.write_all(&record(
            json!({
                "event":"response",
                "provider_attempt":0,
                "elapsed_ms":1,
                "response":{"usage":{"prompt_tokens":20,"completion_tokens":1}},
                "raw_response":null
            }),
            1,
        ))?;
        let usage = cursor.poll(file.path(), 90, true)?;
        assert_eq!(usage.known_input_tokens, 30);
        assert_eq!(usage.missing_cached_input_usage, 1);
        Ok(())
    }

    #[test]
    fn hedges_are_counted_separately_and_cancellation_clears_activity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&record(
            json!({
                "event": "request",
                "provider_attempt": 0,
                "endpoint": "test",
                "model": "test",
                "request": {},
                "options": {},
                "logical_request_id": 7,
                "hedge": {
                    "logical_request_id": 7,
                    "hedge_number": 1,
                    "threshold_ms": 8_000,
                    "p80_ms": 1_000
                }
            }),
            0,
        ))?;
        let mut cursor = TraceCursor::default();
        let usage = cursor.poll(file.path(), 90, true)?;
        assert_eq!(usage.hedges, 1);
        assert_eq!(usage.retries, 0);
        assert_eq!(usage.active_requests, 1);

        file.write_all(&record(
            json!({"event":"cancelled", "reason":"hedge won"}),
            1,
        ))?;
        let usage = cursor.poll(file.path(), 90, true)?;
        assert_eq!(usage.active_requests, 0);
        assert_eq!(usage.hedges, 1);
        Ok(())
    }
}
