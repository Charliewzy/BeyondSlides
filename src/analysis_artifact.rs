use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    AnnotationDiagnostics, PassageProjectionDiagnostics, RestoredAnalysisValidationError,
    RestoredLecturePassage, RestoredTranscript, ValidatedRestoredAnalysis, ValidatedSources,
};

/// Persisted output of a complete restored-transcript analysis run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoredAnalysisArtifact {
    pub restored_transcript: RestoredTranscript,
    pub passages: Vec<RestoredLecturePassage>,
    pub window_diagnostics: Vec<AnnotationDiagnostics>,
    pub window_projections: Vec<PassageProjectionDiagnostics>,
}

impl RestoredAnalysisArtifact {
    /// Revalidates persisted output against its raw transcript and slide deck.
    pub fn validate(
        self,
        sources: ValidatedSources,
    ) -> Result<ValidatedRestoredAnalysis, RestoredAnalysisArtifactError> {
        let window_count = self.window_diagnostics.len();
        validate_count(
            "window_projections",
            window_count,
            self.window_projections.len(),
        )?;
        for (window_index, projection) in self.window_projections.iter().enumerate() {
            if projection.compared_characters == 0 {
                return Err(RestoredAnalysisArtifactError::EmptyProjection { window_index });
            }
        }

        ValidatedRestoredAnalysis::new(sources, self.restored_transcript, self.passages)
            .map_err(RestoredAnalysisArtifactError::Analysis)
    }
}

fn validate_count(
    field: &'static str,
    expected: usize,
    actual: usize,
) -> Result<(), RestoredAnalysisArtifactError> {
    if actual != expected {
        return Err(RestoredAnalysisArtifactError::WindowCountMismatch {
            field,
            expected,
            actual,
        });
    }
    Ok(())
}

#[derive(Debug)]
pub enum RestoredAnalysisArtifactError {
    WindowCountMismatch {
        field: &'static str,
        expected: usize,
        actual: usize,
    },
    EmptyProjection {
        window_index: usize,
    },
    Analysis(RestoredAnalysisValidationError),
}

impl fmt::Display for RestoredAnalysisArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowCountMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "analysis artifact expected {expected} {field} entries but contains {actual}"
            ),
            Self::EmptyProjection { window_index } => write!(
                formatter,
                "analysis artifact window {window_index} has an empty passage projection"
            ),
            Self::Analysis(error) => error.fmt(formatter),
        }
    }
}

impl Error for RestoredAnalysisArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Analysis(error) => Some(error),
            Self::WindowCountMismatch { .. } | Self::EmptyProjection { .. } => None,
        }
    }
}
