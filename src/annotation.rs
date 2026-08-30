use std::{error::Error, fmt};

use serde::Deserialize;

use crate::{
    LecturePassage, LecturePassages, SentenceId, ValidatedAnalysis, ValidatedSources,
    ValidationError, WindowingConfig, build_windows,
};

/// The untrusted structured response produced for one transcript window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TranscriptWindowAnalysis {
    pub passages: Vec<LecturePassage>,
}

/// Assembles one response per transcript window into a validated lecture analysis.
///
/// Responses must be supplied in transcript-window order using the same windowing
/// configuration that created the model tasks. Each response must partition only
/// its owned region; left and right context cannot appear in its lecture passages.
pub fn assemble_window_analyses(
    sources: ValidatedSources,
    windowing_config: WindowingConfig,
    window_analyses: Vec<TranscriptWindowAnalysis>,
) -> Result<ValidatedAnalysis, AnalysisAssemblyError> {
    let passages = {
        let windows = build_windows(&sources, windowing_config);
        let expected_windows = windows.len();
        if window_analyses.len() != expected_windows {
            return Err(AnalysisAssemblyError::WindowCountMismatch {
                expected: expected_windows,
                actual: window_analyses.len(),
            });
        }

        let mut passages = Vec::new();
        for (window_index, (window, analysis)) in windows.iter().zip(window_analyses).enumerate() {
            let owned_region = window.owned_region();
            let owned_start = owned_region
                .first()
                .expect("transcript windows have non-empty owned regions")
                .id;
            let owned_end = owned_region
                .last()
                .expect("transcript windows have non-empty owned regions")
                .id;
            let owned_start_position = owned_start.index();
            let owned_end_position = owned_end.index();
            let mut next_uncovered = 0;
            for passage in analysis.passages {
                let start_position = passage.start.index();
                let end_position = passage.end.index();
                if end_position < start_position {
                    return Err(AnalysisAssemblyError::InvalidAnalysis(
                        ValidationError::PassageEndBeforeStart {
                            start: passage.start,
                            end: passage.end,
                        },
                    ));
                }
                if start_position < owned_start_position || end_position > owned_end_position {
                    return Err(AnalysisAssemblyError::PassageOutsideOwnedRegion {
                        window: window_index + 1,
                        owned_start,
                        owned_end,
                        passage_start: passage.start,
                        passage_end: passage.end,
                    });
                }
                let Some(expected) = owned_region.get(next_uncovered) else {
                    return Err(AnalysisAssemblyError::UnexpectedWindowPassage {
                        window: window_index + 1,
                        actual: passage.start,
                    });
                };
                if passage.start != expected.id {
                    return Err(AnalysisAssemblyError::WindowCoverageMismatch {
                        window: window_index + 1,
                        expected: expected.id,
                        actual: passage.start,
                    });
                }
                next_uncovered = end_position - owned_start_position + 1;
                passages.push(passage);
            }
            if next_uncovered != owned_region.len() {
                return Err(AnalysisAssemblyError::UncoveredWindowTail {
                    window: window_index + 1,
                    expected: owned_region[next_uncovered].id,
                });
            }
        }

        passages
    };
    ValidatedAnalysis::new(sources, LecturePassages { passages })
        .map_err(AnalysisAssemblyError::InvalidAnalysis)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisAssemblyError {
    WindowCountMismatch {
        expected: usize,
        actual: usize,
    },
    PassageOutsideOwnedRegion {
        window: usize,
        owned_start: SentenceId,
        owned_end: SentenceId,
        passage_start: SentenceId,
        passage_end: SentenceId,
    },
    WindowCoverageMismatch {
        window: usize,
        expected: SentenceId,
        actual: SentenceId,
    },
    UnexpectedWindowPassage {
        window: usize,
        actual: SentenceId,
    },
    UncoveredWindowTail {
        window: usize,
        expected: SentenceId,
    },
    InvalidAnalysis(ValidationError),
}

impl fmt::Display for AnalysisAssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowCountMismatch { expected, actual } => write!(
                formatter,
                "analysis expected {expected} transcript window responses but received {actual}"
            ),
            Self::PassageOutsideOwnedRegion {
                window,
                owned_start,
                owned_end,
                passage_start,
                passage_end,
            } => write!(
                formatter,
                "transcript window {window} owns sentences {} through {} but contains a lecture passage from sentence {} through {}",
                owned_start.0, owned_end.0, passage_start.0, passage_end.0
            ),
            Self::WindowCoverageMismatch {
                window,
                expected,
                actual,
            } => write!(
                formatter,
                "transcript window {window} expected coverage at sentence {} but found sentence {}",
                expected.0, actual.0
            ),
            Self::UnexpectedWindowPassage { window, actual } => write!(
                formatter,
                "transcript window {window} contains an unexpected lecture passage starting at sentence {} after its owned region is already covered",
                actual.0
            ),
            Self::UncoveredWindowTail { window, expected } => write!(
                formatter,
                "transcript window {window} leaves sentence {} and the remaining owned-region tail uncovered",
                expected.0
            ),
            Self::InvalidAnalysis(error) => error.fmt(formatter),
        }
    }
}

impl Error for AnalysisAssemblyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidAnalysis(error) => Some(error),
            Self::WindowCountMismatch { .. }
            | Self::PassageOutsideOwnedRegion { .. }
            | Self::WindowCoverageMismatch { .. }
            | Self::UnexpectedWindowPassage { .. }
            | Self::UncoveredWindowTail { .. } => None,
        }
    }
}
