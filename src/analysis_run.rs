use std::{
    error::Error,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    AnnotationDiagnostics, ChatCompletionsClient, DenseSlideScorer, HybridSlideScorer,
    LectureAnalysisConfig, LectureAnalysisProgressError, LectureAnalysisSession, LecturePassage,
    LexicalSlideScorer, SlideDeck, SlideId, Transcript, ValidatedSources, WindowingConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run_support::{
    ProviderSettings, checkpoint_path, display_token_count, initialize_run_directory, read_json,
    read_json_with_hash, sha256, window_progress_bar, write_json_atomically,
};

const ANALYSIS_RUN_FORMAT_VERSION: u32 = 2;
const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;
const MAX_CONCURRENT_WINDOWS: usize = 4;
const MAX_TOOL_ROUNDS: usize = 4;
const MAX_FINAL_ANSWER_REPAIRS: usize = 2;
const MAX_SEARCH_RESULTS: usize = 5;
const MAX_OUTPUT_TOKENS: u32 = 16_384;
const DENSE_MODEL: &str = "BAAI/bge-small-zh-v1.5";
const RETRIEVAL_MODE: &str = "hybrid-rrf";
const ANALYSIS_FILE: &str = "analysis.json";

pub async fn run_canary(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    output_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let output_path = PathBuf::from(output_path);
    let provider = ProviderSettings::from_environment()?;
    let transcript: Transcript = read_json(&transcript_path, "transcript")?;
    let slide_deck: SlideDeck = read_json(&slides_path, "slides")?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;

    eprintln!(
        "Indexing {} slides for hybrid retrieval...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);
    let client = analysis_client(&provider)?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session =
        LectureAnalysisSession::prepare(&client, sources, &hybrid, lecture_config()?)?;
    eprintln!("Sending transcript window 1 as the canary...");
    let canary = session.analyze_canary().await?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot run a canary for an empty transcript",
        )
    })?;

    write_json_atomically(&output_path, canary, "canary analysis")?;
    println!(
        "Wrote {} canary passages to {} ({} tool rounds, {} final-answer repairs, prompt tokens: {}, completion tokens: {})",
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
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let run_directory = PathBuf::from(run_directory);
    let provider = ProviderSettings::from_environment()?;
    let (transcript, transcript_hash) = read_json_with_hash(&transcript_path, "transcript")?;
    let (slide_deck, slides_hash) = read_json_with_hash(&slides_path, "slides")?;
    let manifest = AnalysisRunManifest::new(&provider, transcript_hash, slides_hash);
    initialize_run_directory(&run_directory, &manifest, "analysis")?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;

    eprintln!(
        "Indexing {} slides for hybrid retrieval...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);
    let client = analysis_client(&provider)?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session =
        LectureAnalysisSession::prepare(&client, sources, &hybrid, lecture_config()?)?;
    restore_checkpoints(&mut session, &run_directory)?;

    let progress = window_progress_bar(session.window_count(), session.completed_window_count())?;
    if session.window_count() > 0 {
        if session.completed_window_count() == 0 {
            progress.set_message("running canary");
        }
        let canary = session
            .analyze_canary()
            .await?
            .expect("a nonempty transcript has a canary");
        write_json_atomically(
            &checkpoint_path(&run_directory, 1),
            canary,
            "window checkpoint",
        )?;
        progress.set_position(session.completed_window_count() as u64);
    }

    progress.set_message("analyzing transcript windows");
    let result = session
        .complete_analysis_with_progress(|event| {
            write_json_atomically(
                &checkpoint_path(&run_directory, event.window_number),
                event.result,
                "window checkpoint",
            )
            .map_err(|error| Box::new(error) as LectureAnalysisProgressError)?;
            progress.set_position(event.completed_windows as u64);
            progress.set_message(format!("completed window {}", event.window_number));
            Ok(())
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            progress.abandon_with_message("analysis interrupted; validated checkpoints preserved");
            return Err(error.into());
        }
    };

    let output = CompleteAnalysisOutput {
        passages: result.analysis().passages(),
        slide_positions: result.slide_positions(),
        window_diagnostics: result.window_diagnostics(),
    };
    let output_path = run_directory.join(ANALYSIS_FILE);
    write_json_atomically(&output_path, &output, "complete analysis")?;
    progress.finish_with_message("analysis complete");
    println!(
        "Wrote complete lecture analysis to {}",
        output_path.display()
    );
    Ok(())
}

fn analysis_client(provider: &ProviderSettings) -> Result<ChatCompletionsClient, Box<dyn Error>> {
    let config = provider
        .chat_config()?
        .with_max_tool_rounds(MAX_TOOL_ROUNDS)?
        .with_max_final_answer_repairs(MAX_FINAL_ANSWER_REPAIRS)?
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
    annotation_prompt_sha256: String,
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
    max_search_results: usize,
    max_output_tokens: u32,
}

impl AnalysisRunManifest {
    fn new(provider: &ProviderSettings, transcript_sha256: String, slides_sha256: String) -> Self {
        Self {
            format_version: ANALYSIS_RUN_FORMAT_VERSION,
            transcript_sha256,
            slides_sha256,
            annotation_prompt_sha256: sha256(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/prompts/annotation.md"
            ))),
            api_base_url: provider.base_url().into(),
            model: provider.model().into(),
            chat_extra_body: provider.extra_body().cloned(),
            retrieval_mode: RETRIEVAL_MODE.into(),
            dense_model: DENSE_MODEL.into(),
            max_owned_characters: MAX_OWNED_CHARACTERS,
            max_owned_duration_seconds: MAX_OWNED_DURATION_SECONDS,
            context_characters: CONTEXT_CHARACTERS,
            max_concurrent_windows: MAX_CONCURRENT_WINDOWS,
            max_tool_rounds: MAX_TOOL_ROUNDS,
            max_final_answer_repairs: MAX_FINAL_ANSWER_REPAIRS,
            max_search_results: MAX_SEARCH_RESULTS,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        }
    }
}

#[derive(Serialize)]
struct CompleteAnalysisOutput<'a> {
    passages: &'a [LecturePassage],
    slide_positions: &'a [SlideId],
    window_diagnostics: &'a [AnnotationDiagnostics],
}

fn lecture_config() -> Result<LectureAnalysisConfig, Box<dyn Error>> {
    let windowing = WindowingConfig::new(
        MAX_OWNED_CHARACTERS,
        Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
        CONTEXT_CHARACTERS,
    )?;
    Ok(LectureAnalysisConfig::new(
        windowing,
        MAX_CONCURRENT_WINDOWS,
    )?)
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
    fn run_directory_only_resumes_an_identical_manifest() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let provider = ProviderSettings::new(
            "https://example.test/v1",
            "secret-not-persisted",
            "test-model",
        );
        let manifest = AnalysisRunManifest::new(&provider, "transcript".into(), "slides".into());

        initialize_run_directory(directory.path(), &manifest, "analysis")?;
        initialize_run_directory(directory.path(), &manifest, "analysis")?;

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
        );
        let error = initialize_run_directory(directory.path(), &different, "analysis")
            .expect_err("a changed model must not reuse existing checkpoints");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        Ok(())
    }
}
