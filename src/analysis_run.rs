use std::{
    env,
    error::Error,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    AnnotationDiagnostics, ChatCompletionsClient, ChatCompletionsConfig, DenseSlideScorer,
    HybridSlideScorer, LectureAnalysisConfig, LectureAnalysisProgressError, LectureAnalysisSession,
    LecturePassage, LexicalSlideScorer, SlideDeck, SlideId, Transcript, ValidatedSources,
    WindowingConfig,
};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const API_BASE_URL_ENV: &str = "BEYOND_SLIDES_API_BASE_URL";
const API_KEY_ENV: &str = "BEYOND_SLIDES_API_KEY";
const MODEL_ENV: &str = "BEYOND_SLIDES_MODEL";

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
const MANIFEST_FILE: &str = "manifest.json";
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
    let client = provider.client()?;

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
    initialize_run_directory(&run_directory, &manifest)?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;

    eprintln!(
        "Indexing {} slides for hybrid retrieval...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);
    let client = provider.client()?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session =
        LectureAnalysisSession::prepare(&client, sources, &hybrid, lecture_config()?)?;
    restore_checkpoints(&mut session, &run_directory)?;

    let progress = analysis_progress_bar(session.window_count(), session.completed_window_count())?;
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

struct ProviderSettings {
    base_url: String,
    api_key: String,
    model: String,
}

impl ProviderSettings {
    fn from_environment() -> Result<Self, io::Error> {
        Ok(Self {
            base_url: required_environment_variable(API_BASE_URL_ENV)?,
            api_key: required_environment_variable(API_KEY_ENV)?,
            model: required_environment_variable(MODEL_ENV)?,
        })
    }

    fn client(&self) -> Result<ChatCompletionsClient, Box<dyn Error>> {
        let config = ChatCompletionsConfig::new(
            self.base_url.clone(),
            self.api_key.clone(),
            self.model.clone(),
        )?
        .with_max_tool_rounds(MAX_TOOL_ROUNDS)?
        .with_max_final_answer_repairs(MAX_FINAL_ANSWER_REPAIRS)?
        .with_max_search_results(MAX_SEARCH_RESULTS)?
        .with_max_output_tokens(MAX_OUTPUT_TOKENS)?;
        Ok(ChatCompletionsClient::new(config))
    }
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
            api_base_url: provider.base_url.clone(),
            model: provider.model.clone(),
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

fn initialize_run_directory(
    run_directory: &Path,
    expected_manifest: &AnalysisRunManifest,
) -> Result<(), io::Error> {
    fs::create_dir_all(run_directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not create analysis run directory {}: {error}",
                run_directory.display()
            ),
        )
    })?;
    let manifest_path = run_directory.join(MANIFEST_FILE);
    if manifest_path.exists() {
        let actual_manifest: AnalysisRunManifest = read_json(&manifest_path, "run manifest")?;
        if actual_manifest != *expected_manifest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "analysis run manifest {} does not match the current inputs or configuration; use a new run directory",
                    manifest_path.display()
                ),
            ));
        }
    } else {
        write_json_atomically(&manifest_path, expected_manifest, "run manifest")?;
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

fn checkpoint_path(run_directory: &Path, window_number: usize) -> PathBuf {
    run_directory.join(format!("window-{window_number:04}.json"))
}

fn analysis_progress_bar(
    total_windows: usize,
    completed_windows: usize,
) -> Result<ProgressBar, indicatif::style::TemplateError> {
    let progress = ProgressBar::new(total_windows as u64);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
             {pos}/{len} windows {msg}",
        )?
        .progress_chars("=>-"),
    );
    progress.set_position(completed_windows as u64);
    progress.enable_steady_tick(Duration::from_millis(100));
    Ok(progress)
}

fn required_environment_variable(name: &str) -> Result<String, io::Error> {
    env::var(name).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("could not read required environment variable {name}: {error}"),
        )
    })
}

fn display_token_count(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "unknown".into(), |tokens| tokens.to_string())
}

fn read_json<T: DeserializeOwned>(path: &Path, kind: &str) -> Result<T, io::Error> {
    let bytes = fs::read(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {kind} {}: {error}", path.display()),
        )
    })?;
    parse_json(&bytes, path, kind)
}

fn read_json_with_hash<T: DeserializeOwned>(
    path: &Path,
    kind: &str,
) -> Result<(T, String), io::Error> {
    let bytes = fs::read(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {kind} {}: {error}", path.display()),
        )
    })?;
    let value = parse_json(&bytes, path, kind)?;
    Ok((value, sha256(&bytes)))
}

fn parse_json<T: DeserializeOwned>(bytes: &[u8], path: &Path, kind: &str) -> Result<T, io::Error> {
    serde_json::from_slice(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {kind} JSON in {}: {error}", path.display()),
        )
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_json_atomically<T: Serialize>(
    path: &Path,
    value: &T,
    kind: &str,
) -> Result<(), io::Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not create temporary {kind} in {}: {error}",
                parent.display()
            ),
        )
    })?;
    serde_json::to_writer_pretty(&mut temporary, value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not serialize {kind}: {error}"),
        )
    })?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| {
        io::Error::new(
            error.error.kind(),
            format!(
                "could not persist {kind} {}: {}",
                path.display(),
                error.error
            ),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_directory_only_resumes_an_identical_manifest() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let provider = ProviderSettings {
            base_url: "https://example.test/v1".into(),
            api_key: "secret-not-persisted".into(),
            model: "test-model".into(),
        };
        let manifest = AnalysisRunManifest::new(&provider, "transcript".into(), "slides".into());

        initialize_run_directory(directory.path(), &manifest)?;
        initialize_run_directory(directory.path(), &manifest)?;

        let persisted = fs::read_to_string(directory.path().join(MANIFEST_FILE))?;
        assert!(!persisted.contains(&provider.api_key));

        let different = AnalysisRunManifest::new(
            &ProviderSettings {
                model: "different-model".into(),
                ..provider
            },
            "transcript".into(),
            "slides".into(),
        );
        let error = initialize_run_directory(directory.path(), &different)
            .expect_err("a changed model must not reuse existing checkpoints");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        Ok(())
    }

    #[test]
    fn atomic_json_writer_replaces_an_existing_checkpoint() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("window-0001.json");

        write_json_atomically(&path, &1, "test checkpoint")?;
        write_json_atomically(&path, &2, "test checkpoint")?;

        assert_eq!(read_json::<u8>(&path, "test checkpoint")?, 2);
        Ok(())
    }
}
