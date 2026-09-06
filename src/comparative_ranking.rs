use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroUsize,
};

use crate::processing::{BatchRunError, StopSignal, run_bounded};
use serde::{Deserialize, Serialize};

use crate::{
    AnnotationDiagnostics, ChatCompletionsClient, ChatCompletionsError, ComparativeScore,
    ComparativeScoreError, SearchError, SlideId, SlideScorer, ValidatedRestoredAnalysis,
};

const IMPORTANCE_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/comparative_importance.md"
));
const NOVELTY_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/comparative_novelty.md"
));

const COMPARISON_SIZE: usize = 4;
const DEFAULT_ROUNDS: usize = 8;
const DEFAULT_IMPORTANCE_COMPARISONS_PER_BATCH: usize = 16;
const DEFAULT_NOVELTY_COMPARISONS_PER_BATCH: usize = 8;
const DEFAULT_RETRIEVAL_CANDIDATES: usize = 5;
const DEFAULT_SLIDE_NEIGHBORHOOD_RADIUS: usize = 3;
const DEFAULT_SEED: u64 = 20_260_905;

pub type ComparativeRankingProgressError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparativeMetric {
    Importance,
    Novelty,
}

impl ComparativeMetric {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Importance => "importance",
            Self::Novelty => "novelty",
        }
    }

    const fn instructions(self) -> &'static str {
        match self {
            Self::Importance => IMPORTANCE_INSTRUCTIONS,
            Self::Novelty => NOVELTY_INSTRUCTIONS,
        }
    }
}

/// Product policy for lecture-wide best--worst comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComparativeRankingConfig {
    rounds: usize,
    importance_comparisons_per_batch: NonZeroUsize,
    novelty_comparisons_per_batch: NonZeroUsize,
    max_concurrent_batches: NonZeroUsize,
    retrieval_candidates: usize,
    slide_neighborhood_radius: usize,
    seed: u64,
}

impl ComparativeRankingConfig {
    pub fn new(max_concurrent_batches: usize) -> Result<Self, ComparativeRankingConfigError> {
        let Some(max_concurrent_batches) = NonZeroUsize::new(max_concurrent_batches) else {
            return Err(ComparativeRankingConfigError::ZeroConcurrency);
        };
        Ok(Self {
            rounds: DEFAULT_ROUNDS,
            importance_comparisons_per_batch: NonZeroUsize::new(
                DEFAULT_IMPORTANCE_COMPARISONS_PER_BATCH,
            )
            .expect("the default importance batch size is nonzero"),
            novelty_comparisons_per_batch: NonZeroUsize::new(DEFAULT_NOVELTY_COMPARISONS_PER_BATCH)
                .expect("the default novelty batch size is nonzero"),
            max_concurrent_batches,
            retrieval_candidates: DEFAULT_RETRIEVAL_CANDIDATES,
            slide_neighborhood_radius: DEFAULT_SLIDE_NEIGHBORHOOD_RADIUS,
            seed: DEFAULT_SEED,
        })
    }

    pub fn with_rounds(mut self, rounds: usize) -> Result<Self, ComparativeRankingConfigError> {
        if rounds < 2 || !rounds.is_multiple_of(2) {
            return Err(ComparativeRankingConfigError::InvalidRounds(rounds));
        }
        self.rounds = rounds;
        Ok(self)
    }

    pub fn with_comparisons_per_batch(
        mut self,
        importance: usize,
        novelty: usize,
    ) -> Result<Self, ComparativeRankingConfigError> {
        self.importance_comparisons_per_batch = NonZeroUsize::new(importance).ok_or(
            ComparativeRankingConfigError::ZeroComparisonsPerBatch(ComparativeMetric::Importance),
        )?;
        self.novelty_comparisons_per_batch = NonZeroUsize::new(novelty).ok_or(
            ComparativeRankingConfigError::ZeroComparisonsPerBatch(ComparativeMetric::Novelty),
        )?;
        Ok(self)
    }

    pub const fn with_evidence_limits(
        mut self,
        retrieval_candidates: usize,
        slide_neighborhood_radius: usize,
    ) -> Self {
        self.retrieval_candidates = retrieval_candidates;
        self.slide_neighborhood_radius = slide_neighborhood_radius;
        self
    }

    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub const fn rounds(self) -> usize {
        self.rounds
    }

    pub const fn importance_comparisons_per_batch(self) -> usize {
        self.importance_comparisons_per_batch.get()
    }

    pub const fn novelty_comparisons_per_batch(self) -> usize {
        self.novelty_comparisons_per_batch.get()
    }

    pub const fn max_concurrent_batches(self) -> usize {
        self.max_concurrent_batches.get()
    }

    pub const fn retrieval_candidates(self) -> usize {
        self.retrieval_candidates
    }

    pub const fn slide_neighborhood_radius(self) -> usize {
        self.slide_neighborhood_radius
    }

    pub const fn seed(self) -> u64 {
        self.seed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparativeRankingConfigError {
    ZeroConcurrency,
    InvalidRounds(usize),
    ZeroComparisonsPerBatch(ComparativeMetric),
}

impl fmt::Display for ComparativeRankingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroConcurrency => {
                formatter.write_str("comparative ranking requires at least one concurrent task")
            }
            Self::InvalidRounds(rounds) => write!(
                formatter,
                "comparative ranking rounds must be an even number of at least two, not {rounds}"
            ),
            Self::ZeroComparisonsPerBatch(metric) => write!(
                formatter,
                "{} comparative ranking batches must contain at least one comparison",
                metric.name()
            ),
        }
    }
}

impl Error for ComparativeRankingConfigError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PlannedComparison {
    comparison_id: usize,
    passage_ids: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ComparisonPassageInput {
    #[serde(skip)]
    passage_id: usize,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    slide_position: Option<SlideId>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    candidate_slide_ids: Vec<SlideId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ComparisonSlideInput {
    slide_id: SlideId,
    text: String,
}

#[derive(Serialize)]
struct ComparisonTaskInput<'a> {
    lecture_context: &'a str,
    comparisons: Vec<ComparisonInput<'a>>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    slides: &'a [ComparisonSlideInput],
}

#[derive(Serialize)]
struct ComparisonInput<'a> {
    comparison_id: usize,
    candidates: BTreeMap<CandidateLabel, &'a ComparisonPassageInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
enum CandidateLabel {
    A,
    B,
    C,
    D,
}

impl CandidateLabel {
    const ALL: [Self; COMPARISON_SIZE] = [Self::A, Self::B, Self::C, Self::D];

    const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::C => 2,
            Self::D => 3,
        }
    }
}

fn slice_is_empty<T>(values: &&[T]) -> bool {
    values.is_empty()
}

/// One model request containing independent best--worst comparisons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComparativeRankingTask {
    task_index: usize,
    batch_index: usize,
    metric: ComparativeMetric,
    lecture_context: String,
    comparisons: Vec<PlannedComparison>,
    passages: Vec<ComparisonPassageInput>,
    slides: Vec<ComparisonSlideInput>,
}

impl ComparativeRankingTask {
    pub(crate) const fn task_index(&self) -> usize {
        self.task_index
    }

    pub(crate) const fn batch_index(&self) -> usize {
        self.batch_index
    }

    pub(crate) const fn metric(&self) -> ComparativeMetric {
        self.metric
    }

    pub(crate) fn message(&self) -> Result<ComparativeRankingMessage, serde_json::Error> {
        let passages: BTreeMap<_, _> = self
            .passages
            .iter()
            .map(|passage| (passage.passage_id, passage))
            .collect();
        Ok(ComparativeRankingMessage {
            instructions: self.metric.instructions(),
            input: serde_json::to_string(&ComparisonTaskInput {
                lecture_context: &self.lecture_context,
                comparisons: self
                    .comparisons
                    .iter()
                    .map(|comparison| ComparisonInput {
                        comparison_id: comparison.comparison_id,
                        candidates: CandidateLabel::ALL
                            .into_iter()
                            .zip(comparison.passage_ids.iter().map(|id| passages[id]))
                            .collect(),
                    })
                    .collect(),
                slides: &self.slides,
            })?,
        })
    }

    pub(crate) fn validate(
        &self,
        proposed: ProposedComparativeRanking,
    ) -> Result<Vec<ComparativeDecision>, ComparativeRankingValidationError> {
        let comparisons: BTreeMap<_, _> = self
            .comparisons
            .iter()
            .map(|comparison| (comparison.comparison_id, comparison))
            .collect();
        let decisions = proposed
            .comparisons
            .into_iter()
            .map(|decision| {
                let comparison = comparisons.get(&decision.comparison_id).ok_or(
                    ComparativeRankingValidationError::UnknownComparison {
                        comparison_id: decision.comparison_id,
                    },
                )?;
                let resolve = |label: CandidateLabel| {
                    comparison.passage_ids.get(label.index()).copied().ok_or(
                        ComparativeRankingValidationError::LabelOutsideComparison {
                            comparison_id: decision.comparison_id,
                            label: format!("{label:?}"),
                            candidate_count: comparison.passage_ids.len(),
                        },
                    )
                };
                Ok(ComparativeDecision {
                    comparison_id: decision.comparison_id,
                    most: resolve(decision.most)?,
                    least: resolve(decision.least)?,
                })
            })
            .collect::<Result<Vec<_>, ComparativeRankingValidationError>>()?;
        self.validate_decisions(decisions)
    }

    fn validate_decisions(
        &self,
        decisions: Vec<ComparativeDecision>,
    ) -> Result<Vec<ComparativeDecision>, ComparativeRankingValidationError> {
        let expected: BTreeMap<_, _> = self
            .comparisons
            .iter()
            .map(|comparison| (comparison.comparison_id, comparison))
            .collect();
        let mut actual = BTreeMap::new();
        for decision in decisions {
            let Some(comparison) = expected.get(&decision.comparison_id) else {
                return Err(ComparativeRankingValidationError::UnknownComparison {
                    comparison_id: decision.comparison_id,
                });
            };
            if actual.contains_key(&decision.comparison_id) {
                return Err(ComparativeRankingValidationError::DuplicateComparison {
                    comparison_id: decision.comparison_id,
                });
            }
            if !comparison.passage_ids.contains(&decision.most) {
                return Err(
                    ComparativeRankingValidationError::PassageOutsideComparison {
                        comparison_id: decision.comparison_id,
                        passage_id: decision.most,
                    },
                );
            }
            if !comparison.passage_ids.contains(&decision.least) {
                return Err(
                    ComparativeRankingValidationError::PassageOutsideComparison {
                        comparison_id: decision.comparison_id,
                        passage_id: decision.least,
                    },
                );
            }
            if decision.most == decision.least {
                return Err(ComparativeRankingValidationError::SameMostAndLeast {
                    comparison_id: decision.comparison_id,
                    passage_id: decision.most,
                });
            }
            actual.insert(decision.comparison_id, decision);
        }
        if actual.len() != expected.len() {
            let missing = expected
                .keys()
                .find(|comparison_id| !actual.contains_key(comparison_id))
                .copied()
                .expect("different comparison counts imply one missing ID");
            return Err(ComparativeRankingValidationError::MissingComparison {
                comparison_id: missing,
            });
        }
        Ok(self
            .comparisons
            .iter()
            .map(|comparison| {
                actual
                    .remove(&comparison.comparison_id)
                    .expect("all expected comparisons were validated")
            })
            .collect())
    }

    fn validate_result(
        &self,
        mut result: ComparativeRankingBatchResult,
    ) -> Result<ComparativeRankingBatchResult, ComparativeRankingValidationError> {
        if result.metric != self.metric || result.batch_index != self.batch_index {
            return Err(ComparativeRankingValidationError::TaskIdentityMismatch {
                expected_metric: self.metric,
                expected_batch_index: self.batch_index,
                actual_metric: result.metric,
                actual_batch_index: result.batch_index,
            });
        }
        result.comparisons = self.validate_decisions(result.comparisons)?;
        Ok(result)
    }
}

pub(crate) struct ComparativeRankingMessage {
    pub instructions: &'static str,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparativeDecision {
    pub comparison_id: usize,
    pub most: usize,
    pub least: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProposedComparativeRanking {
    comparisons: Vec<ProposedComparativeDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProposedComparativeDecision {
    comparison_id: usize,
    most: CandidateLabel,
    least: CandidateLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparativeRankingBatchResult {
    pub metric: ComparativeMetric,
    pub batch_index: usize,
    pub comparisons: Vec<ComparativeDecision>,
    pub diagnostics: AnnotationDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparativeRankings {
    pub importance: Vec<ComparativeScore>,
    pub novelty: Vec<ComparativeScore>,
}

#[derive(Debug)]
pub struct CompleteComparativeRanking {
    rankings: ComparativeRankings,
    batch_results: Vec<ComparativeRankingBatchResult>,
}

impl CompleteComparativeRanking {
    pub fn rankings(&self) -> &ComparativeRankings {
        &self.rankings
    }

    pub fn batch_results(&self) -> &[ComparativeRankingBatchResult] {
        &self.batch_results
    }

    pub fn into_rankings(self) -> ComparativeRankings {
        self.rankings
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComparativeRankingBatchInfo {
    pub batch_plan_index: usize,
    pub metric: ComparativeMetric,
    pub batch_index: usize,
}

pub struct ComparativeRankingProgress<'a> {
    pub batch: ComparativeRankingBatchInfo,
    pub completed_batches: usize,
    pub total_batches: usize,
    pub result: &'a ComparativeRankingBatchResult,
}

/// Resumable lecture-wide importance and novelty ranking.
pub struct ComparativeRankingSession<'a> {
    stop: StopSignal,
    client: &'a ChatCompletionsClient,
    passage_count: usize,
    config: ComparativeRankingConfig,
    tasks: Vec<ComparativeRankingTask>,
    results: Vec<Option<ComparativeRankingBatchResult>>,
}

impl<'a> ComparativeRankingSession<'a> {
    pub fn prepare(
        client: &'a ChatCompletionsClient,
        analysis: &ValidatedRestoredAnalysis,
        scorer: &dyn SlideScorer,
        config: ComparativeRankingConfig,
    ) -> Result<Self, ComparativeRankingError> {
        let tasks = build_tasks(analysis, scorer, config)?;
        let results = vec![None; tasks.len()];
        Ok(Self {
            stop: StopSignal::default(),
            client,
            passage_count: analysis.passages().len(),
            config,
            tasks,
            results,
        })
    }

    pub fn with_stop_signal(mut self, stop: StopSignal) -> Self {
        self.stop = stop;
        self
    }

    pub fn batch_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn completed_batch_count(&self) -> usize {
        self.results.iter().flatten().count()
    }

    pub fn batch_info(&self, batch_plan_index: usize) -> Option<ComparativeRankingBatchInfo> {
        self.tasks.get(batch_plan_index).map(batch_info)
    }

    pub fn restore_batch_result(
        &mut self,
        batch_plan_index: usize,
        result: ComparativeRankingBatchResult,
    ) -> Result<(), ComparativeRankingError> {
        let Some(stored) = self.results.get(batch_plan_index) else {
            return Err(ComparativeRankingError::UnknownBatchIndex {
                index: batch_plan_index,
                count: self.tasks.len(),
            });
        };
        if stored.is_some() {
            return Err(ComparativeRankingError::BatchAlreadyCompleted {
                index: batch_plan_index,
            });
        }
        let result = self.tasks[batch_plan_index]
            .validate_result(result)
            .map_err(|source| ComparativeRankingError::InvalidCheckpoint {
                batch: batch_info(&self.tasks[batch_plan_index]),
                source,
            })?;
        self.results[batch_plan_index] = Some(result);
        Ok(())
    }

    pub async fn complete(self) -> Result<CompleteComparativeRanking, ComparativeRankingError> {
        self.complete_with_progress(|_| Ok(())).await
    }

    /// Runs one canary for each metric before launching remaining tasks concurrently.
    pub async fn complete_with_progress(
        self,
        mut report_progress: impl FnMut(
            ComparativeRankingProgress<'_>,
        ) -> Result<(), ComparativeRankingProgressError>,
    ) -> Result<CompleteComparativeRanking, ComparativeRankingError> {
        let Self {
            stop,
            client,
            passage_count,
            config,
            tasks,
            mut results,
        } = self;
        let mut completed_batches = results.iter().flatten().count();
        let canaries = [ComparativeMetric::Importance, ComparativeMetric::Novelty]
            .into_iter()
            .filter_map(|metric| tasks.iter().position(|task| task.metric == metric))
            .collect::<Vec<_>>();

        for task_index in canaries {
            if results[task_index].is_some() {
                continue;
            }
            if stop.is_requested() {
                return Err(ComparativeRankingError::Stopped);
            }
            let result = client
                .compare_passages(&tasks[task_index])
                .await
                .map_err(|source| ComparativeRankingError::Model {
                    batch: batch_info(&tasks[task_index]),
                    source,
                })?;
            completed_batches += 1;
            report_progress(ComparativeRankingProgress {
                batch: batch_info(&tasks[task_index]),
                completed_batches,
                total_batches: tasks.len(),
                result: &result,
            })
            .map_err(ComparativeRankingError::Progress)?;
            results[task_index] = Some(result);
        }

        let pending = results
            .iter()
            .enumerate()
            .filter_map(|(task_index, result)| result.is_none().then_some(task_index))
            .collect::<Vec<_>>();
        run_bounded(
            pending,
            config.max_concurrent_batches,
            &stop,
            |task_index| {
                let task = &tasks[task_index];
                async move {
                    client
                        .compare_passages(task)
                        .await
                        .map(|result| (task_index, result))
                        .map_err(|source| ComparativeRankingError::Model {
                            batch: batch_info(task),
                            source,
                        })
                }
            },
            |(task_index, result)| {
                completed_batches += 1;
                report_progress(ComparativeRankingProgress {
                    batch: batch_info(&tasks[task_index]),
                    completed_batches,
                    total_batches: tasks.len(),
                    result: &result,
                })
                .map_err(ComparativeRankingError::Progress)?;
                results[task_index] = Some(result);
                Ok(())
            },
        )
        .await
        .map_err(|error| match error {
            BatchRunError::Stopped => ComparativeRankingError::Stopped,
            BatchRunError::Work(error) => error,
        })?;

        let results = results
            .into_iter()
            .enumerate()
            .map(|(task_index, result)| {
                result.ok_or(ComparativeRankingError::MissingResult {
                    batch: batch_info(&tasks[task_index]),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rankings = aggregate_rankings(passage_count, &tasks, &results)?;
        Ok(CompleteComparativeRanking {
            rankings,
            batch_results: results,
        })
    }
}

fn batch_info(task: &ComparativeRankingTask) -> ComparativeRankingBatchInfo {
    ComparativeRankingBatchInfo {
        batch_plan_index: task.task_index,
        metric: task.metric,
        batch_index: task.batch_index,
    }
}

fn build_tasks(
    analysis: &ValidatedRestoredAnalysis,
    scorer: &dyn SlideScorer,
    config: ComparativeRankingConfig,
) -> Result<Vec<ComparativeRankingTask>, ComparativeRankingError> {
    let passage_count = analysis.passages().len();
    let comparisons = build_comparisons(passage_count, config.rounds, config.seed);
    if comparisons.is_empty() {
        return Ok(Vec::new());
    }
    let novelty_candidates = build_novelty_candidates(analysis, scorer, config)?;
    let lecture_context = analysis
        .slide_deck()
        .slides
        .iter()
        .take(2)
        .map(|slide| slide.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .chars()
        .take(3_000)
        .collect::<String>();
    let mut tasks = Vec::new();
    for (metric, comparisons_per_batch) in [
        (
            ComparativeMetric::Importance,
            config.importance_comparisons_per_batch,
        ),
        (
            ComparativeMetric::Novelty,
            config.novelty_comparisons_per_batch,
        ),
    ] {
        for (batch_index, batch) in comparisons.chunks(comparisons_per_batch.get()).enumerate() {
            let passage_ids = batch
                .iter()
                .flat_map(|comparison| comparison.passage_ids.iter().copied())
                .collect::<BTreeSet<_>>();
            let passages = passage_ids
                .iter()
                .map(|&passage_id| {
                    let passage = &analysis.passages()[passage_id];
                    ComparisonPassageInput {
                        passage_id,
                        text: passage.text.clone(),
                        slide_position: (metric == ComparativeMetric::Novelty)
                            .then_some(passage.slide_position),
                        candidate_slide_ids: if metric == ComparativeMetric::Novelty {
                            novelty_candidates[passage_id].clone()
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect::<Vec<_>>();
            let slide_ids = passages
                .iter()
                .flat_map(|passage| passage.candidate_slide_ids.iter().copied())
                .map(|slide_id| (slide_id.index(), slide_id))
                .collect::<BTreeMap<_, _>>();
            let slides = slide_ids
                .into_values()
                .map(|slide_id| {
                    let slide = analysis
                        .slide_deck()
                        .find(slide_id)
                        .expect("validated candidate slide IDs exist");
                    ComparisonSlideInput {
                        slide_id,
                        text: slide.text.clone(),
                    }
                })
                .collect();
            tasks.push(ComparativeRankingTask {
                task_index: tasks.len(),
                batch_index,
                metric,
                lecture_context: lecture_context.clone(),
                comparisons: batch.to_vec(),
                passages,
                slides,
            });
        }
    }
    Ok(tasks)
}

fn build_novelty_candidates(
    analysis: &ValidatedRestoredAnalysis,
    scorer: &dyn SlideScorer,
    config: ComparativeRankingConfig,
) -> Result<Vec<Vec<SlideId>>, ComparativeRankingError> {
    let slide_count = analysis.slide_deck().slides.len();
    analysis
        .passages()
        .iter()
        .enumerate()
        .map(|(passage_id, passage)| {
            let position = passage.slide_position.index();
            let start = position.saturating_sub(config.slide_neighborhood_radius);
            let end = position
                .saturating_add(config.slide_neighborhood_radius + 1)
                .min(slide_count);
            let mut candidates = (start..end)
                .map(|index| SlideId(index as u32))
                .collect::<Vec<_>>();
            candidates.extend(passage.related_slides.iter().copied());

            candidates.extend(retrieve_candidate_slides(
                analysis,
                scorer,
                passage_id,
                &passage.text,
                config.retrieval_candidates,
            )?);
            candidates.sort_unstable_by_key(|slide_id| slide_id.index());
            candidates.dedup();
            Ok(candidates)
        })
        .collect()
}

fn retrieve_candidate_slides(
    analysis: &ValidatedRestoredAnalysis,
    scorer: &dyn SlideScorer,
    passage_id: usize,
    query: &str,
    limit: usize,
) -> Result<Vec<SlideId>, ComparativeRankingError> {
    let mut scores = scorer
        .score_slides(query)
        .map_err(|source| ComparativeRankingError::SlideScoring { passage_id, source })?;
    let slides = &analysis.slide_deck().slides;
    if scores.len() != slides.len() {
        return Err(ComparativeRankingError::SlideScoreCountMismatch {
            passage_id,
            expected: slides.len(),
            actual: scores.len(),
        });
    }
    for (slide, score) in slides.iter().zip(&scores) {
        if score.slide_id != slide.id {
            return Err(ComparativeRankingError::UnexpectedScoredSlide {
                passage_id,
                expected: slide.id,
                actual: score.slide_id,
            });
        }
        if !score.score.is_finite() {
            return Err(ComparativeRankingError::NonFiniteSlideScore {
                passage_id,
                slide: score.slide_id,
            });
        }
    }

    scores.retain(|score| score.score > 0.0);
    scores.sort_unstable_by(|left, right| right.score.total_cmp(&left.score));
    Ok(scores
        .into_iter()
        .take(limit)
        .map(|score| score.slide_id)
        .collect())
}

fn build_comparisons(passage_count: usize, rounds: usize, seed: u64) -> Vec<PlannedComparison> {
    if passage_count < 2 {
        return Vec::new();
    }
    let comparison_size = passage_count.min(COMPARISON_SIZE);
    let mut comparisons = Vec::new();
    for round in 0..rounds {
        let mut passage_ids = (0..passage_count).collect::<Vec<_>>();
        shuffle(&mut passage_ids, seed.wrapping_add(round as u64));
        let missing = (comparison_size - passage_ids.len() % comparison_size) % comparison_size;
        passage_ids.extend_from_within(..missing);
        for passage_ids in passage_ids.chunks(comparison_size) {
            comparisons.push(PlannedComparison {
                comparison_id: comparisons.len(),
                passage_ids: passage_ids.to_vec(),
            });
        }
    }
    comparisons
}

fn shuffle(values: &mut [usize], seed: u64) {
    let mut state = seed;
    for end in (1..values.len()).rev() {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut random = state;
        random = (random ^ (random >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        random = (random ^ (random >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        random ^= random >> 31;
        values.swap(end, random as usize % (end + 1));
    }
}

#[derive(Clone, Copy, Default)]
struct SelectionCounts {
    comparisons: u32,
    most: u32,
    least: u32,
}

fn aggregate_rankings(
    passage_count: usize,
    tasks: &[ComparativeRankingTask],
    results: &[ComparativeRankingBatchResult],
) -> Result<ComparativeRankings, ComparativeRankingError> {
    let importance =
        aggregate_metric(ComparativeMetric::Importance, passage_count, tasks, results)?;
    let novelty = aggregate_metric(ComparativeMetric::Novelty, passage_count, tasks, results)?;
    Ok(ComparativeRankings {
        importance,
        novelty,
    })
}

fn aggregate_metric(
    metric: ComparativeMetric,
    passage_count: usize,
    tasks: &[ComparativeRankingTask],
    results: &[ComparativeRankingBatchResult],
) -> Result<Vec<ComparativeScore>, ComparativeRankingError> {
    if passage_count == 0 {
        return Ok(Vec::new());
    }
    if passage_count == 1 {
        return Ok(vec![
            ComparativeScore::new(0, 0, 0, 5_000).expect("a neutral single-passage score is valid"),
        ]);
    }

    let mut counts = vec![SelectionCounts::default(); passage_count];
    for (task, result) in tasks.iter().zip(results) {
        if task.metric != metric {
            continue;
        }
        for (comparison, decision) in task.comparisons.iter().zip(&result.comparisons) {
            for &passage_id in &comparison.passage_ids {
                counts[passage_id].comparisons += 1;
            }
            counts[decision.most].most += 1;
            counts[decision.least].least += 1;
        }
    }

    let mut ranked = (0..passage_count).collect::<Vec<_>>();
    ranked.sort_unstable_by(|&left, &right| compare_counts(counts[left], counts[right]));
    let mut percentiles = vec![0_u16; passage_count];
    let mut start = 0;
    while start < ranked.len() {
        let mut end = start + 1;
        while end < ranked.len()
            && compare_counts(counts[ranked[start]], counts[ranked[end]]) == Ordering::Equal
        {
            end += 1;
        }
        let doubled_average_rank = start + end - 1;
        let denominator = ranked.len() - 1;
        let basis_points = (doubled_average_rank * 5_000 + denominator / 2) / denominator;
        for &passage_id in &ranked[start..end] {
            percentiles[passage_id] = u16::try_from(basis_points)
                .expect("a comparative percentile is at most 10000 basis points");
        }
        start = end;
    }

    counts
        .into_iter()
        .zip(percentiles)
        .map(|(counts, percentile)| {
            ComparativeScore::new(counts.comparisons, counts.most, counts.least, percentile)
                .map_err(ComparativeRankingError::InvalidScore)
        })
        .collect()
}

fn compare_counts(left: SelectionCounts, right: SelectionCounts) -> Ordering {
    let left_balance = i128::from(left.most) - i128::from(left.least);
    let right_balance = i128::from(right.most) - i128::from(right.least);
    (left_balance * i128::from(right.comparisons))
        .cmp(&(right_balance * i128::from(left.comparisons)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparativeRankingValidationError {
    TaskIdentityMismatch {
        expected_metric: ComparativeMetric,
        expected_batch_index: usize,
        actual_metric: ComparativeMetric,
        actual_batch_index: usize,
    },
    UnknownComparison {
        comparison_id: usize,
    },
    DuplicateComparison {
        comparison_id: usize,
    },
    MissingComparison {
        comparison_id: usize,
    },
    PassageOutsideComparison {
        comparison_id: usize,
        passage_id: usize,
    },
    LabelOutsideComparison {
        comparison_id: usize,
        label: String,
        candidate_count: usize,
    },
    SameMostAndLeast {
        comparison_id: usize,
        passage_id: usize,
    },
}

impl fmt::Display for ComparativeRankingValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskIdentityMismatch {
                expected_metric,
                expected_batch_index,
                actual_metric,
                actual_batch_index,
            } => write!(
                formatter,
                "expected {} comparison batch {expected_batch_index} but received {} batch {actual_batch_index}",
                expected_metric.name(),
                actual_metric.name()
            ),
            Self::UnknownComparison { comparison_id } => {
                write!(formatter, "unknown comparison {comparison_id}")
            }
            Self::DuplicateComparison { comparison_id } => {
                write!(
                    formatter,
                    "comparison {comparison_id} was returned more than once"
                )
            }
            Self::MissingComparison { comparison_id } => {
                write!(formatter, "comparison {comparison_id} is missing")
            }
            Self::PassageOutsideComparison {
                comparison_id,
                passage_id,
            } => write!(
                formatter,
                "comparison {comparison_id} selected passage {passage_id}, which is not a member"
            ),
            Self::LabelOutsideComparison {
                comparison_id,
                label,
                candidate_count,
            } => write!(
                formatter,
                "comparison {comparison_id} has only {candidate_count} candidates, so label {label} is not available"
            ),
            Self::SameMostAndLeast {
                comparison_id,
                passage_id,
            } => write!(
                formatter,
                "comparison {comparison_id} selected passage {passage_id} as both most and least"
            ),
        }
    }
}

impl Error for ComparativeRankingValidationError {}

#[derive(Debug)]
pub enum ComparativeRankingError {
    Stopped,
    SlideScoring {
        passage_id: usize,
        source: SearchError,
    },
    SlideScoreCountMismatch {
        passage_id: usize,
        expected: usize,
        actual: usize,
    },
    UnexpectedScoredSlide {
        passage_id: usize,
        expected: SlideId,
        actual: SlideId,
    },
    NonFiniteSlideScore {
        passage_id: usize,
        slide: SlideId,
    },
    UnknownBatchIndex {
        index: usize,
        count: usize,
    },
    BatchAlreadyCompleted {
        index: usize,
    },
    InvalidCheckpoint {
        batch: ComparativeRankingBatchInfo,
        source: ComparativeRankingValidationError,
    },
    Model {
        batch: ComparativeRankingBatchInfo,
        source: ChatCompletionsError,
    },
    MissingResult {
        batch: ComparativeRankingBatchInfo,
    },
    Progress(ComparativeRankingProgressError),
    InvalidScore(ComparativeScoreError),
}

impl fmt::Display for ComparativeRankingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => {
                formatter.write_str("comparative ranking stopped; completed checkpoints preserved")
            }
            Self::SlideScoring { passage_id, source } => write!(
                formatter,
                "could not retrieve novelty evidence for passage {passage_id}: {source}"
            ),
            Self::SlideScoreCountMismatch {
                passage_id,
                expected,
                actual,
            } => write!(
                formatter,
                "novelty retrieval for passage {passage_id} expected {expected} slide scores but received {actual}"
            ),
            Self::UnexpectedScoredSlide {
                passage_id,
                expected,
                actual,
            } => write!(
                formatter,
                "novelty retrieval for passage {passage_id} expected slide {} but received slide {}",
                expected.0, actual.0
            ),
            Self::NonFiniteSlideScore { passage_id, slide } => write!(
                formatter,
                "novelty retrieval for passage {passage_id} returned a non-finite score for slide {}",
                slide.0
            ),
            Self::UnknownBatchIndex { index, count } => write!(
                formatter,
                "comparative ranking batch-plan index {index} does not exist; the plan has {count} batches"
            ),
            Self::BatchAlreadyCompleted { index } => {
                write!(
                    formatter,
                    "comparative ranking batch-plan entry {index} is already complete"
                )
            }
            Self::InvalidCheckpoint { batch, source } => write!(
                formatter,
                "persisted {} comparison batch {} is invalid: {source}",
                batch.metric.name(),
                batch.batch_index
            ),
            Self::Model { batch, source } => write!(
                formatter,
                "could not run {} comparison batch {}: {source}",
                batch.metric.name(),
                batch.batch_index
            ),
            Self::MissingResult { batch } => write!(
                formatter,
                "{} comparison batch {} has no result",
                batch.metric.name(),
                batch.batch_index
            ),
            Self::Progress(error) => {
                write!(formatter, "comparative ranking progress failed: {error}")
            }
            Self::InvalidScore(error) => write!(formatter, "invalid comparative score: {error}"),
        }
    }
}

impl Error for ComparativeRankingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SlideScoring { source, .. } => Some(source),
            Self::InvalidCheckpoint { source, .. } => Some(source),
            Self::Model { source, .. } => Some(source),
            Self::Progress(error) => Some(error.as_ref()),
            Self::InvalidScore(error) => Some(error),
            Self::SlideScoreCountMismatch { .. }
            | Self::UnexpectedScoredSlide { .. }
            | Self::NonFiniteSlideScore { .. }
            | Self::UnknownBatchIndex { .. }
            | Self::BatchAlreadyCompleted { .. }
            | Self::Stopped
            | Self::MissingResult { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_plan_is_reproducible_and_covers_every_passage_each_round() {
        let first = build_comparisons(9, 8, 42);
        let second = build_comparisons(9, 8, 42);
        assert_eq!(first, second);

        let comparisons_per_round = 9_usize.div_ceil(COMPARISON_SIZE);
        assert_eq!(first.len(), comparisons_per_round * 8);
        for round in first.chunks(comparisons_per_round) {
            let covered = round
                .iter()
                .flat_map(|comparison| comparison.passage_ids.iter().copied())
                .collect::<BTreeSet<_>>();
            assert_eq!(covered, (0..9).collect());
            assert!(round.iter().all(|comparison| {
                comparison.passage_ids.iter().collect::<BTreeSet<_>>().len()
                    == comparison.passage_ids.len()
            }));
        }
    }

    #[test]
    fn task_validation_reorders_answers_and_rejects_nonmembers() {
        let task = ranking_task(
            ComparativeMetric::Importance,
            vec![planned(7, [0, 1, 2, 3]), planned(8, [4, 5, 6, 7])],
        );
        let decisions = task
            .validate_decisions(vec![decision(8, 4, 7), decision(7, 2, 0)])
            .expect("valid comparisons");
        assert_eq!(decisions, vec![decision(7, 2, 0), decision(8, 4, 7)]);

        let checkpoint = task
            .validate_result(ComparativeRankingBatchResult {
                metric: ComparativeMetric::Importance,
                batch_index: 0,
                comparisons: vec![decision(8, 4, 7), decision(7, 2, 0)],
                diagnostics: AnnotationDiagnostics::default(),
            })
            .expect("valid checkpoint");
        assert_eq!(
            checkpoint.comparisons,
            vec![decision(7, 2, 0), decision(8, 4, 7)]
        );

        let error = task
            .validate_decisions(vec![decision(7, 9, 0), decision(8, 4, 7)])
            .expect_err("passage 9 is outside comparison 7");
        assert_eq!(
            error,
            ComparativeRankingValidationError::PassageOutsideComparison {
                comparison_id: 7,
                passage_id: 9,
            }
        );
    }

    #[test]
    fn labels_are_local_to_each_group_and_checkpoints_keep_global_ids() {
        let task = ranking_task(
            ComparativeMetric::Importance,
            vec![
                planned(7, [228, 99, 275, 219]),
                planned(8, [35, 256, 0, 190]),
            ],
        );
        let proposed = serde_json::from_value(serde_json::json!({"comparisons":[
            {"comparison_id":8,"most":"A","least":"C"},
            {"comparison_id":7,"most":"A","least":"D"}
        ]}))
        .unwrap();
        let result = task.validate(proposed).unwrap();
        assert_eq!(result, vec![decision(7, 228, 219), decision(8, 35, 0)]);
        assert_eq!(serde_json::to_value(&result).unwrap()[0]["most"], 228);

        for invalid in [serde_json::json!(99), serde_json::json!("E")] {
            assert!(
                serde_json::from_value::<ProposedComparativeRanking>(serde_json::json!({
                    "comparisons":[{"comparison_id":7,"most":invalid,"least":"D"}]
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn labels_must_exist_in_short_groups_and_most_must_differ_from_least() {
        let task = ranking_task(ComparativeMetric::Importance, vec![planned(42, [20, 10])]);
        for (most, least) in [("C", "A"), ("B", "B")] {
            let proposed = serde_json::from_value(serde_json::json!({"comparisons":[
                {"comparison_id":42,"most":most,"least":least}
            ]}))
            .unwrap();
            assert!(task.validate(proposed).is_err());
        }
    }

    #[test]
    fn best_worst_counts_become_percentiles_and_display_levels() {
        let comparisons = vec![planned(0, [0, 1, 2, 3])];
        let tasks = vec![
            ranking_task(ComparativeMetric::Importance, comparisons.clone()),
            ranking_task(ComparativeMetric::Novelty, comparisons),
        ];
        let results = vec![
            batch_result(ComparativeMetric::Importance, vec![decision(0, 0, 3)]),
            batch_result(ComparativeMetric::Novelty, vec![decision(0, 2, 1)]),
        ];

        let rankings = aggregate_rankings(4, &tasks, &results).expect("valid aggregation");
        assert_eq!(levels(&rankings.importance), vec![5, 3, 3, 1]);
        assert_eq!(levels(&rankings.novelty), vec![3, 1, 5, 3]);
        assert_eq!(rankings.importance[0].percentile_basis_points, 10_000);
        assert_eq!(rankings.importance[3].percentile_basis_points, 0);
        assert_eq!(rankings.importance[0].best_worst_score(), 1.0);
        assert_eq!(rankings.importance[3].best_worst_score(), -1.0);
    }

    fn planned<const N: usize>(comparison_id: usize, passage_ids: [usize; N]) -> PlannedComparison {
        PlannedComparison {
            comparison_id,
            passage_ids: passage_ids.into(),
        }
    }

    fn decision(comparison_id: usize, most: usize, least: usize) -> ComparativeDecision {
        ComparativeDecision {
            comparison_id,
            most,
            least,
        }
    }

    fn ranking_task(
        metric: ComparativeMetric,
        comparisons: Vec<PlannedComparison>,
    ) -> ComparativeRankingTask {
        ComparativeRankingTask {
            task_index: 0,
            batch_index: 0,
            metric,
            lecture_context: String::new(),
            comparisons,
            passages: Vec::new(),
            slides: Vec::new(),
        }
    }

    fn batch_result(
        metric: ComparativeMetric,
        comparisons: Vec<ComparativeDecision>,
    ) -> ComparativeRankingBatchResult {
        ComparativeRankingBatchResult {
            metric,
            batch_index: 0,
            comparisons,
            diagnostics: AnnotationDiagnostics::default(),
        }
    }

    fn levels(scores: &[ComparativeScore]) -> Vec<u8> {
        scores
            .iter()
            .map(|score| score.display_level.get())
            .collect()
    }
}
