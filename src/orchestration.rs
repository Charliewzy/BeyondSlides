use std::{error::Error, fmt, num::NonZeroUsize};

use crate::processing::{BatchRunError, StopSignal, run_bounded};

use crate::{
    AnnotationDiagnostics, ChatCompletionsError, LectureModelBackend, PassageProjectionDiagnostics,
    RestoredAnalysisAssemblyError, RestoredAnnotationError, RestoredAnnotationResult,
    RestoredTranscript, RestoredWindowingError, SearchError, SlideAlignmentError, SlideId,
    SlideScorer, ValidatedRestoredAnalysis, ValidatedSources, WindowingConfig,
    assemble_restored_window_analyses, build_restored_annotation_tasks, build_restored_windows,
    infer_slide_positions, validate_restored_window_analysis,
};

pub type LectureAnalysisProgressError = Box<dyn Error + Send + Sync>;

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
    analysis: ValidatedRestoredAnalysis,
    slide_positions: Vec<SlideId>,
    window_diagnostics: Vec<AnnotationDiagnostics>,
    window_projections: Vec<PassageProjectionDiagnostics>,
}

/// One newly completed transcript window and the overall run progress.
pub struct LectureAnalysisProgress<'a> {
    pub window_index: usize,
    pub completed_windows: usize,
    pub total_windows: usize,
    pub result: &'a RestoredAnnotationResult,
}

impl LectureAnalysisResult {
    pub fn analysis(&self) -> &ValidatedRestoredAnalysis {
        &self.analysis
    }

    pub fn slide_positions(&self) -> &[SlideId] {
        &self.slide_positions
    }

    pub fn window_diagnostics(&self) -> &[AnnotationDiagnostics] {
        &self.window_diagnostics
    }

    pub fn window_projections(&self) -> &[PassageProjectionDiagnostics] {
        &self.window_projections
    }

    pub fn into_analysis(self) -> ValidatedRestoredAnalysis {
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
    stop: StopSignal,
    client: &'a dyn LectureModelBackend,
    scorer: &'a dyn SlideScorer,
    sources: ValidatedSources,
    restored_transcript: RestoredTranscript,
    config: LectureAnalysisConfig,
    slide_positions: Vec<SlideId>,
    window_results: Vec<Option<RestoredAnnotationResult>>,
}

impl<'a> LectureAnalysisSession<'a> {
    /// Prepares a lecture analysis without sending any model requests.
    pub fn prepare(
        client: &'a dyn LectureModelBackend,
        sources: ValidatedSources,
        restored_transcript: RestoredTranscript,
        scorer: &'a dyn SlideScorer,
        config: LectureAnalysisConfig,
    ) -> Result<Self, LectureAnalysisError> {
        let windows = build_restored_windows(&sources, &restored_transcript, config.windowing)
            .map_err(LectureAnalysisError::Windowing)?;
        let score_rows = windows
            .iter()
            .enumerate()
            .map(|(position, window)| {
                scorer.score_slides(&window.owned_text()).map_err(|source| {
                    LectureAnalysisError::Scoring {
                        window_index: position,
                        source,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let slide_positions =
            infer_slide_positions(&score_rows).map_err(LectureAnalysisError::Alignment)?;
        let tasks = build_restored_annotation_tasks(&sources, &windows, &slide_positions)
            .map_err(LectureAnalysisError::TaskConstruction)?;
        let window_results = vec![None; tasks.len()];

        Ok(Self {
            stop: StopSignal::default(),
            client,
            scorer,
            sources,
            restored_transcript,
            config,
            slide_positions,
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

    /// Restores one persisted result after validating it against its transcript window.
    ///
    /// `window_index` is the zero-based position used by this prepared session.
    pub fn restore_window_result(
        &mut self,
        window_index: usize,
        result: RestoredAnnotationResult,
    ) -> Result<(), LectureAnalysisError> {
        let Some(stored_result) = self.window_results.get(window_index) else {
            return Err(LectureAnalysisError::UnknownWindowIndex {
                index: window_index,
                count: self.window_results.len(),
            });
        };
        if stored_result.is_some() {
            return Err(LectureAnalysisError::WindowAlreadyCompleted {
                index: window_index,
            });
        }

        let windows = build_restored_windows(
            &self.sources,
            &self.restored_transcript,
            self.config.windowing,
        )
        .map_err(LectureAnalysisError::Windowing)?;
        let tasks = build_restored_annotation_tasks(&self.sources, &windows, &self.slide_positions)
            .map_err(LectureAnalysisError::TaskConstruction)?;
        validate_restored_window_analysis(&self.sources, &tasks[window_index], &result.analysis)
            .map_err(|source| LectureAnalysisError::InvalidRestoredWindow {
                index: window_index,
                source,
            })?;
        self.window_results[window_index] = Some(result);
        Ok(())
    }

    /// Analyzes the first transcript window without launching later windows.
    ///
    /// Repeated calls return the already validated result without sending the
    /// first window again. An empty transcript has no canary and returns `None`.
    pub async fn analyze_canary(
        &mut self,
    ) -> Result<Option<&RestoredAnnotationResult>, LectureAnalysisError> {
        let Some(canary_result) = self.window_results.first() else {
            return Ok(None);
        };
        if canary_result.is_none() {
            if self.stop.is_requested() {
                return Err(LectureAnalysisError::Stopped);
            }
            let result = {
                let windows = build_restored_windows(
                    &self.sources,
                    &self.restored_transcript,
                    self.config.windowing,
                )
                .map_err(LectureAnalysisError::Windowing)?;
                let tasks =
                    build_restored_annotation_tasks(&self.sources, &windows, &self.slide_positions)
                        .map_err(LectureAnalysisError::TaskConstruction)?;
                let Some(canary) = tasks.first() else {
                    return Ok(None);
                };

                self.client
                    .annotate_restored_window(&self.sources, self.scorer, canary)
                    .await
                    .map_err(|source| LectureAnalysisError::WindowAnnotation {
                        window_index: canary.window_index(),
                        source,
                    })?
            };
            self.window_results[0] = Some(result);
        }

        Ok(self.window_results[0].as_ref())
    }

    /// Completes a canary-validated lecture analysis with bounded concurrency.
    pub async fn complete_analysis(self) -> Result<LectureAnalysisResult, LectureAnalysisError> {
        self.complete_analysis_with_progress(|_| Ok(())).await
    }

    /// Completes an analysis while reporting each newly finished transcript window.
    ///
    /// Restored results count toward overall progress but are not reported again.
    pub async fn complete_analysis_with_progress(
        self,
        mut report_progress: impl FnMut(
            LectureAnalysisProgress<'_>,
        ) -> Result<(), LectureAnalysisProgressError>,
    ) -> Result<LectureAnalysisResult, LectureAnalysisError> {
        let Self {
            stop,
            client,
            scorer,
            sources,
            restored_transcript,
            config,
            slide_positions,
            window_results: stored_results,
        } = self;
        let windows = build_restored_windows(&sources, &restored_transcript, config.windowing)
            .map_err(LectureAnalysisError::Windowing)?;
        let tasks = build_restored_annotation_tasks(&sources, &windows, &slide_positions)
            .map_err(LectureAnalysisError::TaskConstruction)?;

        let mut window_results = Vec::with_capacity(tasks.len());
        let mut pending_tasks = Vec::new();
        let mut completed_windows = stored_results.iter().flatten().count();
        for (task, result) in tasks.iter().copied().zip(stored_results) {
            if let Some(result) = result {
                window_results.push((task.window_index(), result));
            } else if task.window_index() == 0 {
                return Err(LectureAnalysisError::CanaryNotAnalyzed);
            } else {
                pending_tasks.push(task);
            }
        }

        if !pending_tasks.is_empty() {
            let sources_ref = &sources;
            run_bounded(
                pending_tasks,
                config.max_concurrent_windows,
                &stop,
                move |task| async move {
                    client
                        .annotate_restored_window(sources_ref, scorer, &task)
                        .await
                        .map(|result| (task.window_index(), result))
                        .map_err(|source| LectureAnalysisError::WindowAnnotation {
                            window_index: task.window_index(),
                            source,
                        })
                },
                |(window_index, result)| {
                    completed_windows += 1;
                    report_progress(LectureAnalysisProgress {
                        window_index,
                        completed_windows,
                        total_windows: tasks.len(),
                        result: &result,
                    })
                    .map_err(LectureAnalysisError::Progress)?;
                    window_results.push((window_index, result));
                    Ok(())
                },
            )
            .await
            .map_err(|error| match error {
                BatchRunError::Stopped => LectureAnalysisError::Stopped,
                BatchRunError::Work(error) => error,
            })?;
        }

        window_results.sort_unstable_by_key(|(window_index, _)| *window_index);
        let mut window_analyses = Vec::with_capacity(window_results.len());
        let mut window_diagnostics = Vec::with_capacity(window_results.len());
        let mut window_projections = Vec::with_capacity(window_results.len());
        for (_, result) in window_results {
            window_analyses.push(result.analysis);
            window_diagnostics.push(result.diagnostics);
            window_projections.push(result.projection);
        }

        drop(tasks);
        drop(windows);
        let analysis = assemble_restored_window_analyses(
            sources,
            restored_transcript,
            config.windowing,
            &slide_positions,
            window_analyses,
        )
        .map_err(LectureAnalysisError::Assembly)?;

        Ok(LectureAnalysisResult {
            analysis,
            slide_positions,
            window_diagnostics,
            window_projections,
        })
    }
}

#[derive(Debug)]
pub enum LectureAnalysisError {
    Stopped,
    Windowing(RestoredWindowingError),
    Scoring {
        window_index: usize,
        source: SearchError,
    },
    Alignment(SlideAlignmentError),
    TaskConstruction(RestoredAnnotationError),
    WindowAnnotation {
        window_index: usize,
        source: ChatCompletionsError,
    },
    UnknownWindowIndex {
        index: usize,
        count: usize,
    },
    WindowAlreadyCompleted {
        index: usize,
    },
    InvalidRestoredWindow {
        index: usize,
        source: RestoredAnnotationError,
    },
    CanaryNotAnalyzed,
    Progress(LectureAnalysisProgressError),
    Assembly(RestoredAnalysisAssemblyError),
}

impl fmt::Display for LectureAnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => {
                formatter.write_str("passage preparation stopped; completed checkpoints preserved")
            }
            Self::Windowing(error) => write!(formatter, "could not window restored text: {error}"),
            Self::Scoring {
                window_index,
                source,
            } => {
                write!(
                    formatter,
                    "could not score slides for restored transcript window at index {window_index}: {source}"
                )
            }
            Self::Alignment(error) => write!(formatter, "could not infer slide positions: {error}"),
            Self::TaskConstruction(error) => {
                write!(formatter, "could not build annotation tasks: {error}")
            }
            Self::WindowAnnotation {
                window_index,
                source,
            } => {
                write!(
                    formatter,
                    "could not annotate restored transcript window at index {window_index}: {source}"
                )
            }
            Self::UnknownWindowIndex { index, count } => {
                write!(
                    formatter,
                    "restored transcript window index {index} does not exist; the lecture has {count} windows"
                )
            }
            Self::WindowAlreadyCompleted { index } => {
                write!(
                    formatter,
                    "restored transcript window index {index} already has a result"
                )
            }
            Self::InvalidRestoredWindow { index, source } => {
                write!(
                    formatter,
                    "persisted result for restored transcript window index {index} is invalid: {source}"
                )
            }
            Self::CanaryNotAnalyzed => {
                formatter.write_str("the canary must succeed before completing lecture analysis")
            }
            Self::Progress(error) => {
                write!(
                    formatter,
                    "could not record lecture analysis progress: {error}"
                )
            }
            Self::Assembly(error) => {
                write!(
                    formatter,
                    "could not assemble restored-transcript-window analyses: {error}"
                )
            }
        }
    }
}

impl Error for LectureAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Windowing(error) => Some(error),
            Self::Scoring { source, .. } => Some(source),
            Self::Alignment(error) => Some(error),
            Self::TaskConstruction(error) => Some(error),
            Self::WindowAnnotation { source, .. } => Some(source),
            Self::UnknownWindowIndex { .. } | Self::WindowAlreadyCompleted { .. } => None,
            Self::InvalidRestoredWindow { source, .. } => Some(source),
            Self::CanaryNotAnalyzed | Self::Stopped => None,
            Self::Progress(error) => Some(error.as_ref()),
            Self::Assembly(error) => Some(error),
        }
    }
}
