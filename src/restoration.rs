use std::{error::Error, fmt, num::NonZeroUsize};

use crate::processing::{BatchRunError, StopSignal, run_bounded};
use serde::{Deserialize, Serialize};

use crate::{
    ChatCompletionsError, LectureModelBackend, RestorationDiagnostics, RestoredTranscript,
    RestoredTranscriptSpan, SlideDeck, Transcript, TranscriptSegment, TranscriptSegmentId,
    TranscriptWindow, TranscriptWindowRestorationResult, ValidatedSources, ValidationError,
    WindowingConfig, build_windows,
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
        let input = TranscriptRestorationInput {
            output_contract: RestorationOutputContract {
                required_source_ids: self.owned_region.iter().map(|segment| segment.id).collect(),
            },
            left_context: self.left_context,
            owned_region: self.owned_region,
            right_context: self.right_context,
        };
        Ok(RestorationMessage {
            instructions: RESTORATION_INSTRUCTIONS,
            input: serde_json::to_string(&input)?,
        })
    }
}

#[derive(Serialize)]
struct TranscriptRestorationInput<'a> {
    output_contract: RestorationOutputContract,
    left_context: &'a [TranscriptSegment],
    owned_region: &'a [TranscriptSegment],
    right_context: &'a [TranscriptSegment],
}

#[derive(Serialize)]
struct RestorationOutputContract {
    required_source_ids: Vec<TranscriptSegmentId>,
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

pub type RestorationProgressError = Box<dyn Error + Send + Sync>;

/// Runtime policy for restoring one complete transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRestorationConfig {
    windowing: WindowingConfig,
    max_concurrent_windows: NonZeroUsize,
}

impl TranscriptRestorationConfig {
    /// Configures transcript windowing and bounded model concurrency.
    pub fn new(
        windowing: WindowingConfig,
        max_concurrent_windows: usize,
    ) -> Result<Self, TranscriptRestorationConfigError> {
        let Some(max_concurrent_windows) = NonZeroUsize::new(max_concurrent_windows) else {
            return Err(TranscriptRestorationConfigError::ZeroConcurrency);
        };
        Ok(Self {
            windowing,
            max_concurrent_windows,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRestorationConfigError {
    ZeroConcurrency,
}

impl fmt::Display for TranscriptRestorationConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroConcurrency => formatter
                .write_str("transcript restoration must allow at least one concurrent window"),
        }
    }
}

impl Error for TranscriptRestorationConfigError {}

/// The restored transcript and diagnostics produced for each source window.
#[derive(Debug)]
pub struct CompleteTranscriptRestoration {
    transcript: RestoredTranscript,
    window_diagnostics: Vec<RestorationDiagnostics>,
}

impl CompleteTranscriptRestoration {
    pub fn transcript(&self) -> &RestoredTranscript {
        &self.transcript
    }

    pub fn window_diagnostics(&self) -> &[RestorationDiagnostics] {
        &self.window_diagnostics
    }

    pub fn into_transcript(self) -> RestoredTranscript {
        self.transcript
    }
}

/// One newly restored transcript window and the overall run progress.
pub struct RestorationProgress<'a> {
    pub window_index: usize,
    pub completed_windows: usize,
    pub total_windows: usize,
    pub result: &'a TranscriptWindowRestorationResult,
}

/// A prepared restoration with validated checkpoint slots for every window.
pub struct TranscriptRestorationSession<'a> {
    stop: StopSignal,
    client: &'a dyn LectureModelBackend,
    sources: ValidatedSources,
    config: TranscriptRestorationConfig,
    window_results: Vec<Option<TranscriptWindowRestorationResult>>,
}

impl<'a> TranscriptRestorationSession<'a> {
    /// Validates and windows the transcript without contacting the model.
    pub fn prepare(
        client: &'a dyn LectureModelBackend,
        transcript: Transcript,
        config: TranscriptRestorationConfig,
    ) -> Result<Self, RestorationSessionError> {
        let sources = ValidatedSources::new(transcript, SlideDeck { slides: Vec::new() })
            .map_err(RestorationSessionError::InvalidTranscript)?;
        let window_results = vec![None; build_windows(&sources, config.windowing).len()];
        Ok(Self {
            stop: StopSignal::default(),
            client,
            sources,
            config,
            window_results,
        })
    }

    pub fn with_stop_signal(mut self, stop: StopSignal) -> Self {
        self.stop = stop;
        self
    }

    pub fn window_count(&self) -> usize {
        self.window_results.len()
    }

    pub fn completed_window_count(&self) -> usize {
        self.window_results.iter().flatten().count()
    }

    /// Loads one persisted result after validating it against its source window.
    pub fn restore_window_checkpoint(
        &mut self,
        window_index: usize,
        result: TranscriptWindowRestorationResult,
    ) -> Result<(), RestorationSessionError> {
        let Some(stored_result) = self.window_results.get(window_index) else {
            return Err(RestorationSessionError::UnknownWindowIndex {
                index: window_index,
                count: self.window_results.len(),
            });
        };
        if stored_result.is_some() {
            return Err(RestorationSessionError::WindowAlreadyCompleted {
                index: window_index,
            });
        }

        let windows = build_windows(&self.sources, self.config.windowing);
        let tasks = build_restoration_tasks(&windows);
        validate_window_restoration(&tasks[window_index], &result.restoration).map_err(
            |source| RestorationSessionError::InvalidWindowCheckpoint {
                index: window_index,
                source,
            },
        )?;
        self.window_results[window_index] = Some(result);
        Ok(())
    }

    pub async fn complete_restoration(
        self,
    ) -> Result<CompleteTranscriptRestoration, RestorationSessionError> {
        self.complete_restoration_with_progress(|_| Ok(())).await
    }

    /// Restores all missing windows with bounded concurrency.
    /// Persisted results count toward progress but are not reported again.
    pub async fn complete_restoration_with_progress(
        self,
        mut report_progress: impl FnMut(RestorationProgress<'_>) -> Result<(), RestorationProgressError>,
    ) -> Result<CompleteTranscriptRestoration, RestorationSessionError> {
        let Self {
            stop,
            client,
            sources,
            config,
            window_results: stored_results,
        } = self;
        let windows = build_windows(&sources, config.windowing);
        let tasks = build_restoration_tasks(&windows);
        let mut window_results = Vec::with_capacity(tasks.len());
        let mut pending_tasks = Vec::new();
        let mut completed_windows = stored_results.iter().flatten().count();

        for (task, result) in tasks.iter().copied().zip(stored_results) {
            if let Some(result) = result {
                window_results.push((task.window_index, result));
            } else {
                pending_tasks.push(task);
            }
        }

        if !pending_tasks.is_empty() {
            run_bounded(
                pending_tasks,
                config.max_concurrent_windows,
                &stop,
                move |task| async move {
                    client
                        .restore_window(&task)
                        .await
                        .map(|result| (task.window_index, result))
                        .map_err(|source| RestorationSessionError::WindowRestoration {
                            index: task.window_index,
                            source,
                        })
                },
                |(window_index, result)| {
                    completed_windows += 1;
                    report_progress(RestorationProgress {
                        window_index,
                        completed_windows,
                        total_windows: tasks.len(),
                        result: &result,
                    })
                    .map_err(RestorationSessionError::Progress)?;
                    window_results.push((window_index, result));
                    Ok(())
                },
            )
            .await
            .map_err(|error| match error {
                BatchRunError::Stopped => RestorationSessionError::Stopped,
                BatchRunError::Work(error) => error,
            })?;
        }

        window_results.sort_unstable_by_key(|(window_index, _)| *window_index);
        let (window_restorations, window_diagnostics) = window_results
            .into_iter()
            .map(|(_, result)| (result.restoration, result.diagnostics))
            .unzip();
        let transcript = assemble_restored_transcript(&windows, window_restorations)
            .map_err(RestorationSessionError::Assembly)?;
        Ok(CompleteTranscriptRestoration {
            transcript,
            window_diagnostics,
        })
    }
}

#[derive(Debug)]
pub enum RestorationSessionError {
    Stopped,
    InvalidTranscript(ValidationError),
    UnknownWindowIndex {
        index: usize,
        count: usize,
    },
    WindowAlreadyCompleted {
        index: usize,
    },
    InvalidWindowCheckpoint {
        index: usize,
        source: RestorationError,
    },
    WindowRestoration {
        index: usize,
        source: ChatCompletionsError,
    },
    Progress(RestorationProgressError),
    Assembly(RestorationError),
}

impl fmt::Display for RestorationSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => {
                formatter.write_str("restoration stopped; completed checkpoints preserved")
            }
            Self::InvalidTranscript(error) => write!(formatter, "invalid transcript: {error}"),
            Self::UnknownWindowIndex { index, count } => write!(
                formatter,
                "transcript window index {index} does not exist; the transcript has {count} windows"
            ),
            Self::WindowAlreadyCompleted { index } => write!(
                formatter,
                "transcript window index {index} already has a restoration result"
            ),
            Self::InvalidWindowCheckpoint { index, source } => write!(
                formatter,
                "persisted restoration for transcript window index {index} is invalid: {source}"
            ),
            Self::WindowRestoration { index, source } => write!(
                formatter,
                "could not restore transcript window index {index}: {source}"
            ),
            Self::Progress(error) => write!(formatter, "restoration progress failed: {error}"),
            Self::Assembly(error) => {
                write!(formatter, "could not assemble restored transcript: {error}")
            }
        }
    }
}

impl Error for RestorationSessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidTranscript(error) => Some(error),
            Self::InvalidWindowCheckpoint { source, .. } | Self::Assembly(source) => Some(source),
            Self::WindowRestoration { source, .. } => Some(source),
            Self::Progress(error) => Some(error.as_ref()),
            Self::UnknownWindowIndex { .. }
            | Self::WindowAlreadyCompleted { .. }
            | Self::Stopped => None,
        }
    }
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
