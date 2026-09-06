use std::{error::Error, fmt};

use crate::{Transcript, TranscriptSegment, TranscriptSegmentId};

const FUNASR_TSV_HEADER: &str = "start\tend\ttext";

/// Converts FunASR's timestamped TSV output into a normalized transcript.
pub fn import_tsv(input: &str) -> Result<Transcript, ImportError> {
    let mut lines = input.lines();
    let header = lines.next().ok_or(ImportError::MissingHeader)?;
    if header != FUNASR_TSV_HEADER {
        return Err(ImportError::InvalidHeader {
            actual: header.to_owned(),
        });
    }

    let mut segments = Vec::new();
    let mut previous_row = None;
    for (position, row) in lines.enumerate() {
        let line = position + 2;
        if row.trim().is_empty() {
            continue;
        }

        let mut fields = row.splitn(3, '\t');
        let (Some(start), Some(end), Some(text)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(ImportError::MalformedRow { line });
        };

        let start_ms =
            parse_milliseconds(start.trim()).ok_or_else(|| ImportError::InvalidTimestamp {
                line,
                field: "start",
                value: start.to_owned(),
            })?;
        let end_ms =
            parse_milliseconds(end.trim()).ok_or_else(|| ImportError::InvalidTimestamp {
                line,
                field: "end",
                value: end.to_owned(),
            })?;
        if end_ms < start_ms {
            return Err(ImportError::EndBeforeStart {
                line,
                start_ms,
                end_ms,
            });
        }
        if let Some((previous_line, previous_end_ms)) = previous_row
            && start_ms < previous_end_ms
        {
            return Err(ImportError::OverlappingRows {
                previous_line,
                line,
                previous_end_ms,
                start_ms,
            });
        }

        let text = text.trim();
        if text.is_empty() {
            return Err(ImportError::EmptyText { line });
        }

        let id = u32::try_from(segments.len())
            .ok()
            .map(TranscriptSegmentId)
            .ok_or(ImportError::TooManyRows)?;
        segments.push(TranscriptSegment {
            id,
            start_ms: Some(start_ms),
            end_ms: Some(end_ms),
            text: text.to_owned(),
        });
        previous_row = Some((line, end_ms));
    }

    Ok(Transcript { segments })
}

fn parse_milliseconds(value: &str) -> Option<u64> {
    let (seconds, fraction) = match value.split_once('.') {
        Some((seconds, fraction)) if !fraction.is_empty() => (seconds, fraction),
        Some(_) => return None,
        None => (value, ""),
    };
    if seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 3
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let seconds = seconds.parse::<u64>().ok()?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().ok()? * 10_u64.pow(3 - fraction.len() as u32)
    };
    seconds.checked_mul(1_000)?.checked_add(fraction)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    MissingHeader,
    InvalidHeader {
        actual: String,
    },
    MalformedRow {
        line: usize,
    },
    InvalidTimestamp {
        line: usize,
        field: &'static str,
        value: String,
    },
    EndBeforeStart {
        line: usize,
        start_ms: u64,
        end_ms: u64,
    },
    OverlappingRows {
        previous_line: usize,
        line: usize,
        previous_end_ms: u64,
        start_ms: u64,
    },
    EmptyText {
        line: usize,
    },
    TooManyRows,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(formatter, "FunASR TSV is missing its header"),
            Self::InvalidHeader { actual } => write!(
                formatter,
                "expected FunASR TSV header {FUNASR_TSV_HEADER:?}, found {actual:?}"
            ),
            Self::MalformedRow { line } => {
                write!(
                    formatter,
                    "FunASR TSV line {line} must contain start, end, and text"
                )
            }
            Self::InvalidTimestamp { line, field, value } => write!(
                formatter,
                "FunASR TSV line {line} has invalid {field} timestamp {value:?}"
            ),
            Self::EndBeforeStart {
                line,
                start_ms,
                end_ms,
            } => write!(
                formatter,
                "FunASR TSV line {line} starts at {start_ms} ms but ends at {end_ms} ms"
            ),
            Self::OverlappingRows {
                previous_line,
                line,
                previous_end_ms,
                start_ms,
            } => write!(
                formatter,
                "FunASR TSV line {line} starts at {start_ms} ms before line {previous_line} ends at {previous_end_ms} ms"
            ),
            Self::EmptyText { line } => {
                write!(formatter, "FunASR TSV line {line} has empty text")
            }
            Self::TooManyRows => write!(
                formatter,
                "FunASR TSV contains more transcript segments than can be assigned IDs"
            ),
        }
    }
}

impl Error for ImportError {}
