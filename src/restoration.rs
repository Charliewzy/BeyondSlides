use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    RestoredTranscript, RestoredTranscriptSpan, TranscriptSegment, TranscriptSegmentId,
    TranscriptWindow,
    windowing::{OwnedRegionPartitionError, validate_owned_region_partition},
};

const RESTORATION_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/restoration.md"
));

/// The source evidence and ownership contract for one transcript-restoration task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TranscriptRestorationTask<'a> {
    #[serde(skip)]
    pub window_index: usize,
    pub left_context: &'a [TranscriptSegment],
    pub owned_region: &'a [TranscriptSegment],
    pub right_context: &'a [TranscriptSegment],
}

impl TranscriptRestorationTask<'_> {
    /// Separates trusted restoration instructions from untrusted transcript text.
    pub fn message(&self) -> Result<RestorationMessage, serde_json::Error> {
        Ok(RestorationMessage {
            instructions: RESTORATION_INSTRUCTIONS,
            input: serde_json::to_string(self)?,
        })
    }
}

/// Provider-neutral content for one transcript-restoration request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorationMessage {
    pub instructions: &'static str,
    pub input: String,
}

/// The untrusted structured response produced for one transcript window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TranscriptWindowRestoration {
    pub spans: Vec<RestoredTranscriptSpan>,
}

pub fn build_restoration_tasks<'a>(
    windows: &[TranscriptWindow<'a>],
) -> Vec<TranscriptRestorationTask<'a>> {
    windows
        .iter()
        .enumerate()
        .map(|(window_index, window)| TranscriptRestorationTask {
            window_index,
            left_context: window.left_context(),
            owned_region: window.owned_region(),
            right_context: window.right_context(),
        })
        .collect()
}

pub fn validate_window_restoration(
    task: &TranscriptRestorationTask<'_>,
    restoration: &TranscriptWindowRestoration,
) -> Result<(), RestorationError> {
    validate_owned_region(task.window_index, task.owned_region, restoration)
}

/// Joins validated window responses without rebuilding their source windows.
pub fn assemble_restored_transcript(
    windows: &[TranscriptWindow<'_>],
    window_restorations: Vec<TranscriptWindowRestoration>,
) -> Result<RestoredTranscript, RestorationError> {
    if window_restorations.len() != windows.len() {
        return Err(RestorationError::WindowCountMismatch {
            expected: windows.len(),
            actual: window_restorations.len(),
        });
    }

    let mut spans = Vec::new();
    for (window_index, (window, restoration)) in windows.iter().zip(window_restorations).enumerate()
    {
        validate_owned_region(window_index, window.owned_region(), &restoration)?;
        spans.extend(restoration.spans);
    }

    Ok(RestoredTranscript { spans })
}

fn validate_owned_region(
    window_index: usize,
    owned_region: &[TranscriptSegment],
    restoration: &TranscriptWindowRestoration,
) -> Result<(), RestorationError> {
    validate_owned_region_partition(
        owned_region,
        restoration
            .spans
            .iter()
            .map(|span| (span.source_start(), span.source_end())),
    )
    .map_err(|error| restoration_partition_error(window_index, error))?;

    for span in &restoration.spans {
        if let RestoredTranscriptSpan::Text { text, .. } = span
            && text.trim().is_empty()
        {
            return Err(RestorationError::EmptyText {
                window_index,
                source_start: span.source_start(),
            });
        }
    }

    Ok(())
}

fn restoration_partition_error(
    window_index: usize,
    error: OwnedRegionPartitionError,
) -> RestorationError {
    match error {
        OwnedRegionPartitionError::EmptyOwnedRegion => {
            RestorationError::EmptyOwnedRegion { window_index }
        }
        OwnedRegionPartitionError::EndBeforeStart { start, end } => {
            RestorationError::SpanEndBeforeStart {
                window_index,
                source_start: start,
                source_end: end,
            }
        }
        OwnedRegionPartitionError::OutsideOwnedRegion {
            owned_start,
            owned_end,
            start,
            end,
        } => RestorationError::SpanOutsideOwnedRegion {
            window_index,
            owned_start,
            owned_end,
            source_start: start,
            source_end: end,
        },
        OwnedRegionPartitionError::CoverageMismatch { expected, actual } => {
            RestorationError::CoverageMismatch {
                window_index,
                expected,
                actual,
            }
        }
        OwnedRegionPartitionError::UnexpectedRange { actual } => RestorationError::UnexpectedSpan {
            window_index,
            source_start: actual,
        },
        OwnedRegionPartitionError::UncoveredTail { expected } => RestorationError::UncoveredTail {
            window_index,
            expected,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorationError {
    WindowCountMismatch {
        expected: usize,
        actual: usize,
    },
    EmptyOwnedRegion {
        window_index: usize,
    },
    SpanEndBeforeStart {
        window_index: usize,
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
    },
    SpanOutsideOwnedRegion {
        window_index: usize,
        owned_start: TranscriptSegmentId,
        owned_end: TranscriptSegmentId,
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
    },
    CoverageMismatch {
        window_index: usize,
        expected: TranscriptSegmentId,
        actual: TranscriptSegmentId,
    },
    UnexpectedSpan {
        window_index: usize,
        source_start: TranscriptSegmentId,
    },
    UncoveredTail {
        window_index: usize,
        expected: TranscriptSegmentId,
    },
    EmptyText {
        window_index: usize,
        source_start: TranscriptSegmentId,
    },
}

impl fmt::Display for RestorationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowCountMismatch { expected, actual } => write!(
                formatter,
                "restoration expected {expected} transcript window responses but received {actual}"
            ),
            Self::EmptyOwnedRegion { window_index } => {
                write!(
                    formatter,
                    "transcript window at index {window_index} has an empty owned region"
                )
            }
            Self::SpanEndBeforeStart {
                window_index,
                source_start,
                source_end,
            } => write!(
                formatter,
                "transcript window at index {window_index} has a restored span ending at segment {} before it starts at segment {}",
                source_end.0, source_start.0,
            ),
            Self::SpanOutsideOwnedRegion {
                window_index,
                owned_start,
                owned_end,
                source_start,
                source_end,
            } => write!(
                formatter,
                "transcript window at index {window_index} owns segments {} through {}, but a restored span covers {} through {}",
                owned_start.0, owned_end.0, source_start.0, source_end.0
            ),
            Self::CoverageMismatch {
                window_index,
                expected,
                actual,
            } => write!(
                formatter,
                "transcript window at index {window_index} expected restoration coverage at segment {} but found segment {}",
                expected.0, actual.0
            ),
            Self::UnexpectedSpan {
                window_index,
                source_start,
            } => write!(
                formatter,
                "transcript window at index {window_index} contains an unexpected restored span starting at segment {} after its owned region is already covered",
                source_start.0
            ),
            Self::UncoveredTail {
                window_index,
                expected,
            } => write!(
                formatter,
                "transcript window at index {window_index} leaves segment {} and the remaining owned-region tail unrestored",
                expected.0
            ),
            Self::EmptyText {
                window_index,
                source_start,
            } => write!(
                formatter,
                "transcript window at index {window_index} has empty restored text starting at segment {}",
                source_start.0
            ),
        }
    }
}

impl Error for RestorationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}
