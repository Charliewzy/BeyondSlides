use std::{error::Error, fmt, num::NonZeroUsize};

use futures::{StreamExt, TryStreamExt, stream};

use crate::{
    AnalysisAssemblyError, AnnotationDiagnostics, AnnotationResult, AnnotationTaskError,
    ChatCompletionsClient, ChatCompletionsError, SearchError, SlideAlignmentError, SlideId,
    SlideScorer, TranscriptWindow, TranscriptWindowAnalysis, ValidatedAnalysis, ValidatedSources,
    WindowingConfig, assemble_window_analyses, build_annotation_tasks, build_windows,
    infer_slide_positions,
};

/// Runtime policy for analyzing one complete lecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LectureAnalysisConfig {
    windowing: WindowingConfig,
    max_concurrent_windows: NonZeroUsize,
}

impl LectureAnalysisConfig {
    /// Configures windowing and post-canary concurrency.
    ///
    /// The first transcript window always runs alone. `max_concurrent_windows`
    /// limits only the remaining windows after that canary succeeds.
    pub fn new(
        windowing: WindowingConfig,
        max_concurrent_windows: usize,
    ) -> Result<Self, LectureAnalysisConfigError> {
        let Some(max_concurrent_windows) = NonZeroUsize::new(max_concurrent_windows) else {
            return Err(LectureAnalysisConfigError::ZeroConcurrency);
        };
        Ok(Self {
            windowing,
            max_concurrent_windows,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LectureAnalysisConfigError {
    ZeroConcurrency,
}

impl fmt::Display for LectureAnalysisConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroConcurrency => {
                formatter.write_str("lecture analysis must allow at least one concurrent window")
            }
        }
    }
}

impl Error for LectureAnalysisConfigError {}

/// The validated analysis and model diagnostics produced for one lecture.
///
/// Slide positions and diagnostics remain in transcript-window order even
/// though non-canary windows may complete out of order.
#[derive(Debug)]
pub struct LectureAnalysisResult {
    analysis: ValidatedAnalysis,
    slide_positions: Vec<SlideId>,
    window_diagnostics: Vec<AnnotationDiagnostics>,
}

impl LectureAnalysisResult {
    pub fn analysis(&self) -> &ValidatedAnalysis {
        &self.analysis
    }

    pub fn slide_positions(&self) -> &[SlideId] {
        &self.slide_positions
    }

    pub fn window_diagnostics(&self) -> &[AnnotationDiagnostics] {
        &self.window_diagnostics
    }

    pub fn into_analysis(self) -> ValidatedAnalysis {
        self.analysis
    }
}

/// A prepared lecture analysis that pauses after its first model request.
///
/// Preparation performs windowing, all-slide scoring, slide-position
/// inference, and annotation-task validation without contacting the model.
/// Call `analyze_canary` to validate the first transcript window in isolation,
/// then call `complete_analysis` to process the remaining windows.
pub struct LectureAnalysisSession<'a> {
    client: &'a ChatCompletionsClient,
    scorer: &'a dyn SlideScorer,
    sources: ValidatedSources,
    config: LectureAnalysisConfig,
    slide_positions: Vec<SlideId>,
    canary_result: Option<AnnotationResult>,
}

impl<'a> LectureAnalysisSession<'a> {
    /// Prepares a lecture analysis without sending any model requests.
    pub fn prepare(
        client: &'a ChatCompletionsClient,
        sources: ValidatedSources,
        scorer: &'a dyn SlideScorer,
        config: LectureAnalysisConfig,
    ) -> Result<Self, LectureAnalysisError> {
        let windows = build_windows(&sources, config.windowing);
        let score_rows = windows
            .iter()
            .enumerate()
            .map(|(position, window)| {
                scorer
                    .score_slides(&window_query(window))
                    .map_err(|source| LectureAnalysisError::Scoring {
                        window: position + 1,
                        source,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let slide_positions =
            infer_slide_positions(&score_rows).map_err(LectureAnalysisError::Alignment)?;
        build_annotation_tasks(&sources, &windows, &slide_positions)
            .map_err(LectureAnalysisError::TaskConstruction)?;

        Ok(Self {
            client,
            scorer,
            sources,
            config,
            slide_positions,
            canary_result: None,
        })
    }

    /// Analyzes the first transcript window without launching later windows.
    ///
    /// Repeated calls return the already validated result without sending the
    /// first window again. An empty transcript has no canary and returns `None`.
    pub async fn analyze_canary(
        &mut self,
    ) -> Result<Option<&AnnotationResult>, LectureAnalysisError> {
        if self.canary_result.is_none() {
            let result = {
                let windows = build_windows(&self.sources, self.config.windowing);
                let tasks = build_annotation_tasks(&self.sources, &windows, &self.slide_positions)
                    .map_err(LectureAnalysisError::TaskConstruction)?;
                let Some(canary) = tasks.first() else {
                    return Ok(None);
                };

                self.client
                    .annotate_window(&self.sources, self.scorer, canary)
                    .await
                    .map_err(|source| LectureAnalysisError::WindowAnnotation {
                        window: canary.window_number,
                        source,
                    })?
            };
            self.canary_result = Some(result);
        }

        Ok(self.canary_result.as_ref())
    }

    /// Completes a canary-validated lecture analysis with bounded concurrency.
    pub async fn complete_analysis(self) -> Result<LectureAnalysisResult, LectureAnalysisError> {
        let Self {
            client,
            scorer,
            sources,
            config,
            slide_positions,
            canary_result,
        } = self;
        let windows = build_windows(&sources, config.windowing);
        let tasks = build_annotation_tasks(&sources, &windows, &slide_positions)
            .map_err(LectureAnalysisError::TaskConstruction)?;

        let mut window_results = Vec::with_capacity(tasks.len());
        if let Some((canary, remaining)) = tasks.split_first() {
            let result = canary_result.ok_or(LectureAnalysisError::CanaryNotAnalyzed)?;
            window_results.push((canary.window_number, result));

            let sources_ref = &sources;
            let remaining_results = stream::iter(remaining.iter().copied())
                .map(move |task| async move {
                    client
                        .annotate_window(sources_ref, scorer, &task)
                        .await
                        .map(|result| (task.window_number, result))
                        .map_err(|source| LectureAnalysisError::WindowAnnotation {
                            window: task.window_number,
                            source,
                        })
                })
                .buffer_unordered(config.max_concurrent_windows.get())
                .try_collect::<Vec<_>>()
                .await?;
            window_results.extend(remaining_results);
        }

        window_results.sort_unstable_by_key(|(window, _)| *window);
        let (window_analyses, window_diagnostics) = window_results
            .into_iter()
            .map(|(_, result)| (result.analysis, result.diagnostics))
            .unzip::<_, _, Vec<TranscriptWindowAnalysis>, Vec<AnnotationDiagnostics>>();

        drop(tasks);
        drop(windows);
        let analysis = assemble_window_analyses(sources, config.windowing, window_analyses)
            .map_err(LectureAnalysisError::Assembly)?;

        Ok(LectureAnalysisResult {
            analysis,
            slide_positions,
            window_diagnostics,
        })
    }
}

fn window_query(window: &TranscriptWindow<'_>) -> String {
    window
        .owned_region()
        .iter()
        .map(|sentence| sentence.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug)]
pub enum LectureAnalysisError {
    Scoring {
        window: usize,
        source: SearchError,
    },
    Alignment(SlideAlignmentError),
    TaskConstruction(AnnotationTaskError),
    WindowAnnotation {
        window: usize,
        source: ChatCompletionsError,
    },
    CanaryNotAnalyzed,
    Assembly(AnalysisAssemblyError),
}

impl fmt::Display for LectureAnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scoring { window, source } => {
                write!(
                    formatter,
                    "could not score slides for transcript window {window}: {source}"
                )
            }
            Self::Alignment(error) => write!(formatter, "could not infer slide positions: {error}"),
            Self::TaskConstruction(error) => {
                write!(formatter, "could not build annotation tasks: {error}")
            }
            Self::WindowAnnotation { window, source } => {
                write!(
                    formatter,
                    "could not annotate transcript window {window}: {source}"
                )
            }
            Self::CanaryNotAnalyzed => {
                formatter.write_str("the canary must succeed before completing lecture analysis")
            }
            Self::Assembly(error) => {
                write!(
                    formatter,
                    "could not assemble transcript-window analyses: {error}"
                )
            }
        }
    }
}

impl Error for LectureAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Scoring { source, .. } => Some(source),
            Self::Alignment(error) => Some(error),
            Self::TaskConstruction(error) => Some(error),
            Self::WindowAnnotation { source, .. } => Some(source),
            Self::CanaryNotAnalyzed => None,
            Self::Assembly(error) => Some(error),
        }
    }
}
