use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    ChatCompletionsClient, ComparativeMetric, ComparativeRankingConfig,
    ComparativeRankingProgressError, ComparativeRankingSession, ContinuousReportMedia,
    DenseSlideScorer, HybridSlideScorer, LectureAnalysisConfig, LectureAnalysisProgressError,
    LectureAnalysisSession, LexicalSlideScorer, ModelExchangeTrace, PlaybackTimingBasis,
    RestoredAnalysisArtifact, RestoredTranscript, SlideDeck, TimedTranscript, Transcript,
    ValidatedRestoredAnalysis, ValidatedSources, WindowingConfig,
    evaluation::{render_annotation_quality, summarize_annotation_quality},
    project_passage_playback_intervals, read_model_trace, render_continuous_report_with_media,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{
    ProviderSettings, checkpoint_path, display_token_count, initialize_run_directory,
    initialize_run_directory_with, model_trace_path, open_output_model_trace, open_run_model_trace,
    ranking_progress_bar, read_json, read_json_with_hash, sha256, window_progress_bar,
    write_json_atomically, write_text_atomically,
};
use crate::{
    report_assets::{prepare_audio_asset, render_pdf_slides},
    restoration_run,
};

const ANALYSIS_RUN_FORMAT_VERSION: u32 = 6;
const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;
const MAX_TOOL_ROUNDS: usize = 4;
const MAX_FINAL_ANSWER_REPAIRS: usize = 2;
const MAX_PROVIDER_RETRIES: usize = 5;
const MAX_SEARCH_RESULTS: usize = 5;
const MAX_OUTPUT_TOKENS: u32 = 16_384;
const COMPARATIVE_ROUNDS: usize = 8;
const IMPORTANCE_COMPARISONS_PER_BATCH: usize = 16;
const NOVELTY_COMPARISONS_PER_BATCH: usize = 8;
const COMPARATIVE_RETRIEVAL_CANDIDATES: usize = 5;
const COMPARATIVE_SLIDE_NEIGHBORHOOD_RADIUS: usize = 3;
const COMPARATIVE_SEED: u64 = 20_260_905;
const DENSE_MODEL: &str = "BAAI/bge-small-zh-v1.5";
const RETRIEVAL_MODE: &str = "hybrid-rrf";
const ANALYSIS_FILE: &str = "analysis.json";
const REPORT_FILE: &str = "report.html";
const QUALITY_FILE: &str = "annotation-quality.json";
const RESTORATION_DIRECTORY: &str = "restoration";
const COMPARISON_DIRECTORY: &str = "comparisons";

pub async fn run_canary(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    output_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let output_path = PathBuf::from(output_path);
    let provider = ProviderSettings::from_annotation_environment()?;
    let restoration_directory = output_path.with_extension("restoration");
    restoration_run::run_complete(
        transcript_path.as_os_str(),
        restoration_directory.as_os_str(),
        Some(provider.scheduler()),
    )
    .await?;
    let transcript: Transcript = read_json(&transcript_path, "transcript")?;
    let slide_deck: SlideDeck = read_json(&slides_path, "slides")?;
    let restored_transcript: RestoredTranscript = read_json(
        &restoration_directory.join(restoration_run::RESTORED_TRANSCRIPT_FILE),
        "restored transcript",
    )?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;

    eprintln!(
        "Indexing {} slides for hybrid retrieval...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);
    let client = analysis_client(&provider, open_output_model_trace(&output_path)?)?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session = LectureAnalysisSession::prepare(
        &client,
        sources,
        restored_transcript,
        &hybrid,
        lecture_config(&provider)?,
    )?;
    eprintln!("Sending transcript window 1 as the passage-preparation canary...");
    let canary = session.analyze_canary().await?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot run a canary for an empty transcript",
        )
    })?;

    write_json_atomically(&output_path, canary, "canary analysis")?;
    println!(
        "Wrote {} preliminary canary passages to {} (importance and novelty are assigned only by a complete run; {} tool rounds, {} final-answer repairs, prompt tokens: {}, completion tokens: {})",
        canary.analysis.passages.len(),
        output_path.display(),
        canary.diagnostics.tool_rounds,
        canary.diagnostics.final_answer_repairs,
        display_token_count(canary.diagnostics.prompt_tokens),
        display_token_count(canary.diagnostics.completion_tokens),
    );
    Ok(())
}

pub async fn run_complete(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    run_directory: &OsStr,
    slide_pdf_path: Option<&OsStr>,
    audio_path: Option<&OsStr>,
    timed_tokens_path: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let run_directory = PathBuf::from(run_directory);
    let slide_pdf_path = slide_pdf_path.map(PathBuf::from);
    let audio_path = audio_path.map(PathBuf::from);
    let timed_tokens_path = timed_tokens_path.map(PathBuf::from);
    let provider = ProviderSettings::from_annotation_environment()?;
    let restoration_directory = run_directory.join(RESTORATION_DIRECTORY);
    restoration_run::run_complete(
        transcript_path.as_os_str(),
        restoration_directory.as_os_str(),
        Some(provider.scheduler()),
    )
    .await?;
    let (transcript, transcript_hash) = read_json_with_hash(&transcript_path, "transcript")?;
    let (slide_deck, slides_hash) = read_json_with_hash(&slides_path, "slides")?;
    let (restored_transcript, restored_transcript_hash) = read_json_with_hash(
        &restoration_directory.join(restoration_run::RESTORED_TRANSCRIPT_FILE),
        "restored transcript",
    )?;
    let manifest = AnalysisRunManifest::new(
        &provider,
        transcript_hash,
        slides_hash,
        restored_transcript_hash,
    );
    initialize_run_directory_with(
        &run_directory,
        &manifest,
        "passage preparation",
        AnalysisRunManifest::preparation_compatible,
    )?;
    provider.record_execution_settings(&run_directory)?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;

    eprintln!(
        "Indexing {} slides for hybrid retrieval...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);
    let client = analysis_client(&provider, open_run_model_trace(&run_directory)?)?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session = LectureAnalysisSession::prepare(
        &client,
        sources,
        restored_transcript,
        &hybrid,
        lecture_config(&provider)?,
    )?;
    restore_checkpoints(&mut session, &run_directory)?;

    let passage_progress =
        window_progress_bar(session.window_count(), session.completed_window_count())?;
    if session.window_count() > 0 {
        if session.completed_window_count() == 0 {
            passage_progress.set_message("running passage-partition canary");
        }
        let canary = session.analyze_canary().await;
        provider.record_scheduling(&run_directory)?;
        let canary = canary?.expect("a nonempty transcript has a canary");
        write_json_atomically(
            &checkpoint_path(&run_directory, 1),
            canary,
            "window checkpoint",
        )?;
        passage_progress.set_position(session.completed_window_count() as u64);
    }

    passage_progress.set_message("preparing lecture passages");
    let result = session
        .complete_analysis_with_progress(|event| {
            let window_number = event.window_index + 1;
            write_json_atomically(
                &checkpoint_path(&run_directory, window_number),
                event.result,
                "window checkpoint",
            )
            .map_err(|error| Box::new(error) as LectureAnalysisProgressError)?;
            passage_progress.set_position(event.completed_windows as u64);
            provider
                .record_scheduling(&run_directory)
                .map_err(|error| Box::new(error) as LectureAnalysisProgressError)?;
            passage_progress.set_message(
                provider.progress_message(&format!("completed window {window_number}")),
            );
            Ok(())
        })
        .await;
    provider.record_scheduling(&run_directory)?;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            passage_progress
                .abandon_with_message("passage preparation interrupted; checkpoints preserved");
            return Err(error.into());
        }
    };
    passage_progress.finish_with_message("passage preparation complete");

    eprintln!("Preparing lecture-wide importance and novelty comparisons...");
    let mut ranking_session = ComparativeRankingSession::prepare(
        &client,
        result.analysis(),
        &hybrid,
        comparative_ranking_config(&provider)?,
    )?;
    let comparison_directories =
        initialize_comparison_directories(&run_directory, &manifest, result.analysis())?;
    restore_comparison_checkpoints(&mut ranking_session, &comparison_directories)?;
    let ranking_progress = ranking_progress_bar(
        ranking_session.batch_count(),
        ranking_session.completed_batch_count(),
    )?;
    ranking_progress.set_message("running metric canaries, then remaining batches");
    let ranking_result = ranking_session
        .complete_with_progress(|event| {
            let path = comparison_checkpoint_path(
                &comparison_directories[&event.batch.metric],
                event.batch.metric,
                event.batch.batch_index,
            );
            write_json_atomically(&path, event.result, "comparison checkpoint")
                .map_err(|error| Box::new(error) as ComparativeRankingProgressError)?;
            ranking_progress.set_position(event.completed_batches as u64);
            provider
                .record_scheduling(&run_directory)
                .map_err(|error| Box::new(error) as ComparativeRankingProgressError)?;
            ranking_progress.set_message(provider.progress_message(&format!(
                "completed {} batch {}",
                event.batch.metric.name(),
                event.batch.batch_index + 1
            )));
            Ok(())
        })
        .await;
    provider.record_scheduling(&run_directory)?;
    let ranking_result = match ranking_result {
        Ok(result) => result,
        Err(error) => {
            ranking_progress.abandon_with_message(
                "comparative ranking interrupted; validated checkpoints preserved",
            );
            return Err(error.into());
        }
    };
    ranking_progress.finish_with_message("comparative ranking complete");

    let window_diagnostics = result.window_diagnostics().to_vec();
    let window_projections = result.window_projections().to_vec();
    let analysis = result
        .into_analysis()
        .with_comparative_rankings(ranking_result.into_rankings())?;

    let output = RestoredAnalysisArtifact {
        restored_transcript: analysis.restored_transcript().clone(),
        passages: analysis.passages().to_vec(),
        window_diagnostics,
        window_projections,
    };
    let output_path = run_directory.join(ANALYSIS_FILE);
    write_json_atomically(&output_path, &output, "complete analysis")?;
    let report_path = run_directory.join(REPORT_FILE);
    write_analysis_report(
        &analysis,
        &report_path,
        slide_pdf_path.as_deref(),
        audio_path.as_deref(),
        timed_tokens_path.as_deref(),
    )?;
    let trace = read_model_trace(&model_trace_path(&run_directory))?;
    let quality = summarize_annotation_quality(&output, &trace);
    let quality_path = run_directory.join(QUALITY_FILE);
    write_json_atomically(&quality_path, &quality, "annotation quality summary")?;
    println!(
        "Wrote complete lecture analysis to {} and report to {}",
        output_path.display(),
        report_path.display()
    );
    print!("{}", render_annotation_quality(&quality));
    Ok(())
}

pub fn render_saved_analysis(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    analysis_path: &OsStr,
    report_path: &OsStr,
    slide_pdf_path: Option<&OsStr>,
    audio_path: Option<&OsStr>,
    timed_tokens_path: Option<&OsStr>,
) -> Result<(), Box<dyn Error>> {
    let transcript: Transcript = read_json(Path::new(transcript_path), "transcript")?;
    let slide_deck: SlideDeck = read_json(Path::new(slides_path), "slides")?;
    let artifact: RestoredAnalysisArtifact =
        read_json(Path::new(analysis_path), "restored analysis")?;
    let analysis = artifact.validate(ValidatedSources::new(transcript, slide_deck)?)?;
    write_analysis_report(
        &analysis,
        Path::new(report_path),
        slide_pdf_path.map(Path::new),
        audio_path.map(Path::new),
        timed_tokens_path.map(Path::new),
    )?;
    println!(
        "Wrote continuous lecture report to {}",
        Path::new(report_path).display()
    );
    Ok(())
}

fn write_analysis_report(
    analysis: &ValidatedRestoredAnalysis,
    report_path: &Path,
    slide_pdf_path: Option<&Path>,
    audio_path: Option<&Path>,
    timed_tokens_path: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    if timed_tokens_path.is_some() && audio_path.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--timed-tokens requires --audio because playback timing has no effect without a recording",
        )
        .into());
    }
    let slide_images = if let Some(slide_pdf_path) = slide_pdf_path {
        render_pdf_slides(
            slide_pdf_path,
            report_path,
            analysis.slide_deck().slides.len(),
        )?
    } else {
        Vec::new()
    };
    let audio = audio_path
        .map(|audio_path| prepare_audio_asset(audio_path, report_path))
        .transpose()?;
    let playback_intervals = timed_tokens_path
        .map(|timed_tokens_path| {
            let timing: TimedTranscript = read_json(timed_tokens_path, "timed transcript")?;
            let intervals = project_passage_playback_intervals(
                analysis.transcript(),
                analysis.passages(),
                &timing,
            )?;
            let token_aligned = intervals
                .iter()
                .filter(|interval| interval.basis == PlaybackTimingBasis::TimedTokens)
                .count();
            eprintln!(
                "Projected token-level playback timing for {token_aligned}/{} passages; the remainder use coarse transcript-segment timing.",
                intervals.len()
            );
            Ok::<_, Box<dyn Error>>(intervals)
        })
        .transpose()?
        .unwrap_or_default();
    let report = render_continuous_report_with_media(
        analysis,
        &ContinuousReportMedia {
            slide_images,
            audio,
            playback_intervals,
        },
    )?;
    write_text_atomically(report_path, &report, "continuous lecture report")?;
    Ok(())
}

pub fn evaluate_saved_analysis(
    analysis_path: &OsStr,
    trace_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let artifact: RestoredAnalysisArtifact =
        read_json(Path::new(analysis_path), "restored analysis")?;
    let trace = read_model_trace(Path::new(trace_path))?;
    print!(
        "{}",
        render_annotation_quality(&summarize_annotation_quality(&artifact, &trace))
    );
    Ok(())
}

fn analysis_client(
    provider: &ProviderSettings,
    model_trace: ModelExchangeTrace,
) -> Result<ChatCompletionsClient, Box<dyn Error>> {
    let config = provider
        .chat_config()?
        .with_model_trace(model_trace)
        .with_max_tool_rounds(MAX_TOOL_ROUNDS)?
        .with_max_final_answer_repairs(MAX_FINAL_ANSWER_REPAIRS)?
        .with_max_provider_retries(MAX_PROVIDER_RETRIES)
        .with_max_search_results(MAX_SEARCH_RESULTS)?
        .with_max_output_tokens(MAX_OUTPUT_TOKENS)?;
    Ok(ChatCompletionsClient::new(config))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnalysisRunManifest {
    format_version: u32,
    transcript_sha256: String,
    slides_sha256: String,
    restored_transcript_sha256: String,
    annotation_prompt_sha256: String,
    importance_comparison_prompt_sha256: String,
    novelty_comparison_prompt_sha256: String,
    api_base_url: String,
    model: String,
    #[serde(default)]
    chat_extra_body: Option<Value>,
    retrieval_mode: String,
    dense_model: String,
    max_owned_characters: usize,
    max_owned_duration_seconds: u64,
    context_characters: usize,
    max_concurrent_windows: usize,
    max_tool_rounds: usize,
    max_final_answer_repairs: usize,
    max_provider_retries: usize,
    minimum_request_interval_seconds: u64,
    max_search_results: usize,
    max_output_tokens: u32,
    comparative_rounds: usize,
    importance_comparisons_per_batch: usize,
    novelty_comparisons_per_batch: usize,
    max_concurrent_comparison_batches: usize,
    comparative_retrieval_candidates: usize,
    comparative_slide_neighborhood_radius: usize,
    comparative_seed: u64,
}

impl AnalysisRunManifest {
    fn new(
        provider: &ProviderSettings,
        transcript_sha256: String,
        slides_sha256: String,
        restored_transcript_sha256: String,
    ) -> Self {
        Self {
            format_version: ANALYSIS_RUN_FORMAT_VERSION,
            transcript_sha256,
            slides_sha256,
            restored_transcript_sha256,
            annotation_prompt_sha256: sha256(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/restored_annotation.md"
            ))),
            importance_comparison_prompt_sha256: sha256(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/comparative_importance.md"
            ))),
            novelty_comparison_prompt_sha256: sha256(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/comparative_novelty.md"
            ))),
            api_base_url: provider.base_url().into(),
            model: provider.model().into(),
            chat_extra_body: provider.extra_body().cloned(),
            retrieval_mode: RETRIEVAL_MODE.into(),
            dense_model: DENSE_MODEL.into(),
            max_owned_characters: MAX_OWNED_CHARACTERS,
            max_owned_duration_seconds: MAX_OWNED_DURATION_SECONDS,
            context_characters: CONTEXT_CHARACTERS,
            max_concurrent_windows: provider.max_concurrency(),
            max_tool_rounds: MAX_TOOL_ROUNDS,
            max_final_answer_repairs: MAX_FINAL_ANSWER_REPAIRS,
            max_provider_retries: MAX_PROVIDER_RETRIES,
            minimum_request_interval_seconds: provider.request_interval().as_secs(),
            max_search_results: MAX_SEARCH_RESULTS,
            max_output_tokens: MAX_OUTPUT_TOKENS,
            comparative_rounds: COMPARATIVE_ROUNDS,
            importance_comparisons_per_batch: IMPORTANCE_COMPARISONS_PER_BATCH,
            novelty_comparisons_per_batch: NOVELTY_COMPARISONS_PER_BATCH,
            max_concurrent_comparison_batches: provider.max_concurrency(),
            comparative_retrieval_candidates: COMPARATIVE_RETRIEVAL_CANDIDATES,
            comparative_slide_neighborhood_radius: COMPARATIVE_SLIDE_NEIGHBORHOOD_RADIUS,
            comparative_seed: COMPARATIVE_SEED,
        }
    }

    fn preparation_identity(&self) -> PreparationIdentity {
        PreparationIdentity {
            format_version: self.format_version,
            transcript_sha256: self.transcript_sha256.clone(),
            slides_sha256: self.slides_sha256.clone(),
            restored_transcript_sha256: self.restored_transcript_sha256.clone(),
            annotation_prompt_sha256: self.annotation_prompt_sha256.clone(),
            api_base_url: self.api_base_url.clone(),
            model: self.model.clone(),
            chat_extra_body: self.chat_extra_body.clone(),
            retrieval_mode: self.retrieval_mode.clone(),
            dense_model: self.dense_model.clone(),
            max_owned_characters: self.max_owned_characters,
            max_owned_duration_seconds: self.max_owned_duration_seconds,
            context_characters: self.context_characters,
            max_tool_rounds: self.max_tool_rounds,
            max_search_results: self.max_search_results,
            max_output_tokens: self.max_output_tokens,
        }
    }

    fn preparation_compatible(&self, expected: &Self) -> bool {
        self.preparation_identity() == expected.preparation_identity()
    }
}

/// The legacy flat manifest remains readable; only these fields determine
/// whether its passage checkpoints can be reused. Runtime policy and the
/// downstream comparison prompts do not belong to this stage's identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparationIdentity {
    format_version: u32,
    transcript_sha256: String,
    slides_sha256: String,
    restored_transcript_sha256: String,
    annotation_prompt_sha256: String,
    api_base_url: String,
    model: String,
    chat_extra_body: Option<Value>,
    retrieval_mode: String,
    dense_model: String,
    max_owned_characters: usize,
    max_owned_duration_seconds: u64,
    context_characters: usize,
    max_tool_rounds: usize,
    max_search_results: usize,
    max_output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonRunManifest {
    format_version: u32,
    preparation: PreparationIdentity,
    passages_sha256: String,
    metric: ComparativeMetric,
    prompt_sha256: String,
    rounds: usize,
    comparisons_per_batch: usize,
    seed: u64,
    evidence_limits: Option<(usize, usize)>,
}

impl ComparisonRunManifest {
    fn new(
        manifest: &AnalysisRunManifest,
        passages_sha256: String,
        metric: ComparativeMetric,
    ) -> Self {
        let (prompt, batch_size) = match metric {
            ComparativeMetric::Importance => (
                &manifest.importance_comparison_prompt_sha256,
                manifest.importance_comparisons_per_batch,
            ),
            ComparativeMetric::Novelty => (
                &manifest.novelty_comparison_prompt_sha256,
                manifest.novelty_comparisons_per_batch,
            ),
        };
        Self {
            format_version: 1,
            preparation: manifest.preparation_identity(),
            passages_sha256,
            metric,
            prompt_sha256: prompt.clone(),
            rounds: manifest.comparative_rounds,
            comparisons_per_batch: batch_size,
            seed: manifest.comparative_seed,
            evidence_limits: (metric == ComparativeMetric::Novelty).then_some((
                manifest.comparative_retrieval_candidates,
                manifest.comparative_slide_neighborhood_radius,
            )),
        }
    }

    fn directory(&self, run_directory: &Path) -> Result<PathBuf, serde_json::Error> {
        let fingerprint = sha256(&serde_json::to_vec(self)?);
        Ok(run_directory
            .join(COMPARISON_DIRECTORY)
            .join(format!("{}-{fingerprint}", self.metric.name())))
    }
}

fn initialize_comparison_directories(
    run_directory: &Path,
    manifest: &AnalysisRunManifest,
    analysis: &ValidatedRestoredAnalysis,
) -> Result<BTreeMap<ComparativeMetric, PathBuf>, Box<dyn Error>> {
    let passages_sha256 = sha256(&serde_json::to_vec(analysis.passages())?);
    [ComparativeMetric::Importance, ComparativeMetric::Novelty]
        .into_iter()
        .map(|metric| {
            let stage = ComparisonRunManifest::new(manifest, passages_sha256.clone(), metric);
            let directory = stage.directory(run_directory)?;
            initialize_run_directory(&directory, &stage, metric.name())?;
            Ok((metric, directory))
        })
        .collect()
}

fn lecture_config(provider: &ProviderSettings) -> Result<LectureAnalysisConfig, Box<dyn Error>> {
    let windowing = WindowingConfig::new(
        MAX_OWNED_CHARACTERS,
        Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
        CONTEXT_CHARACTERS,
    )?;
    Ok(LectureAnalysisConfig::new(
        windowing,
        provider.max_concurrency(),
    )?)
}

fn comparative_ranking_config(
    provider: &ProviderSettings,
) -> Result<ComparativeRankingConfig, Box<dyn Error>> {
    Ok(ComparativeRankingConfig::new(provider.max_concurrency())?
        .with_rounds(COMPARATIVE_ROUNDS)?
        .with_comparisons_per_batch(
            IMPORTANCE_COMPARISONS_PER_BATCH,
            NOVELTY_COMPARISONS_PER_BATCH,
        )?
        .with_evidence_limits(
            COMPARATIVE_RETRIEVAL_CANDIDATES,
            COMPARATIVE_SLIDE_NEIGHBORHOOD_RADIUS,
        )
        .with_seed(COMPARATIVE_SEED))
}

fn comparison_checkpoint_path(
    directory: &Path,
    metric: ComparativeMetric,
    batch_index: usize,
) -> PathBuf {
    directory.join(format!(
        "{}-batch-{:04}.json",
        metric.name(),
        batch_index + 1
    ))
}

fn restore_comparison_checkpoints(
    session: &mut ComparativeRankingSession<'_>,
    directories: &BTreeMap<ComparativeMetric, PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for batch_plan_index in 0..session.batch_count() {
        let batch = session
            .batch_info(batch_plan_index)
            .expect("a batch-plan index below batch_count exists");
        let path = comparison_checkpoint_path(
            &directories[&batch.metric],
            batch.metric,
            batch.batch_index,
        );
        if path.exists() {
            let result = read_json(&path, "comparison checkpoint")?;
            session.restore_batch_result(batch_plan_index, result)?;
        }
    }
    let restored = session.completed_batch_count();
    if restored > 0 {
        eprintln!(
            "Restored {restored}/{} validated comparison batches",
            session.batch_count()
        );
    }
    Ok(())
}

fn restore_checkpoints(
    session: &mut LectureAnalysisSession<'_>,
    run_directory: &Path,
) -> Result<(), Box<dyn Error>> {
    for window_index in 0..session.window_count() {
        let window_number = window_index + 1;
        let path = checkpoint_path(run_directory, window_number);
        if path.exists() {
            let result = read_json(&path, "window checkpoint")?;
            session.restore_window_result(window_index, result)?;
        }
    }
    let restored = session.completed_window_count();
    if restored > 0 {
        eprintln!(
            "Restored {restored}/{} validated windows",
            session.window_count()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn preparation_resume_ignores_only_downstream_and_operational_changes()
    -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let provider = ProviderSettings::new(
            "https://example.test/v1",
            "secret-not-persisted",
            "test-model",
        );
        let manifest = AnalysisRunManifest::new(
            &provider,
            "transcript".into(),
            "slides".into(),
            "restored".into(),
        );

        initialize_run_directory_with(
            directory.path(),
            &manifest,
            "analysis",
            AnalysisRunManifest::preparation_compatible,
        )?;
        let mut runtime_change = manifest.clone();
        runtime_change.max_concurrent_windows = 12;
        runtime_change.minimum_request_interval_seconds = 60;
        runtime_change.max_provider_retries = 10;
        runtime_change.max_final_answer_repairs = 0;
        runtime_change.max_concurrent_comparison_batches = 8;
        runtime_change.importance_comparison_prompt_sha256 = "new-label-prompt".into();
        runtime_change.novelty_comparison_prompt_sha256 = "new-evidence-prompt".into();
        runtime_change.comparative_rounds = 16;
        runtime_change.comparative_seed = 123;
        initialize_run_directory_with(
            directory.path(),
            &runtime_change,
            "analysis",
            AnalysisRunManifest::preparation_compatible,
        )?;

        let persisted = fs::read_to_string(directory.path().join("manifest.json"))?;
        assert!(!persisted.contains("secret-not-persisted"));

        let different = AnalysisRunManifest::new(
            &ProviderSettings::new(
                "https://example.test/v1",
                "secret-not-persisted",
                "different-model",
            ),
            "transcript".into(),
            "slides".into(),
            "restored".into(),
        );
        let error = initialize_run_directory_with(
            directory.path(),
            &different,
            "analysis",
            AnalysisRunManifest::preparation_compatible,
        )
        .expect_err("a changed model must not reuse existing checkpoints");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        for field in [
            "format_version",
            "transcript_sha256",
            "slides_sha256",
            "restored_transcript_sha256",
            "annotation_prompt_sha256",
            "api_base_url",
            "chat_extra_body",
            "max_owned_characters",
            "max_tool_rounds",
            "retrieval_mode",
            "dense_model",
        ] {
            let mut changed = serde_json::to_value(&manifest)?;
            changed[field] = match &changed[field] {
                Value::Number(_) => serde_json::json!(999),
                Value::Null => serde_json::json!({"thinking":{"type":"enabled"}}),
                _ => serde_json::json!("changed"),
            };
            let changed = serde_json::from_value::<AnalysisRunManifest>(changed)?;
            assert!(
                !manifest.preparation_compatible(&changed),
                "{field} must invalidate preparation"
            );
        }
        Ok(())
    }

    #[test]
    fn comparison_checkpoint_namespaces_track_their_own_dependencies() {
        let provider = ProviderSettings::new("https://example.test/v1", "secret", "model");
        let manifest =
            AnalysisRunManifest::new(&provider, "raw".into(), "slides".into(), "restored".into());
        let importance = ComparisonRunManifest::new(
            &manifest,
            "prepared-passages".into(),
            ComparativeMetric::Importance,
        );
        let novelty = ComparisonRunManifest::new(
            &manifest,
            "prepared-passages".into(),
            ComparativeMetric::Novelty,
        );
        let mut changed = manifest.clone();
        changed.max_concurrent_windows = 9;
        changed.max_concurrent_comparison_batches = 7;
        changed.minimum_request_interval_seconds = 1;
        assert_eq!(
            importance,
            ComparisonRunManifest::new(
                &changed,
                "prepared-passages".into(),
                ComparativeMetric::Importance
            )
        );
        assert_eq!(
            novelty,
            ComparisonRunManifest::new(
                &changed,
                "prepared-passages".into(),
                ComparativeMetric::Novelty
            )
        );
        changed.importance_comparison_prompt_sha256 = "new prompt".into();
        let new_importance = ComparisonRunManifest::new(
            &changed,
            "prepared-passages".into(),
            ComparativeMetric::Importance,
        );
        assert_ne!(
            importance.directory(Path::new("run")).unwrap(),
            new_importance.directory(Path::new("run")).unwrap()
        );
        assert_eq!(
            novelty,
            ComparisonRunManifest::new(
                &changed,
                "prepared-passages".into(),
                ComparativeMetric::Novelty
            )
        );
        assert_ne!(
            importance,
            ComparisonRunManifest::new(
                &manifest,
                "different partition".into(),
                ComparativeMetric::Importance
            )
        );
        changed.comparative_retrieval_candidates += 1;
        assert_ne!(
            novelty,
            ComparisonRunManifest::new(
                &changed,
                "prepared-passages".into(),
                ComparativeMetric::Novelty
            )
        );
    }
}
