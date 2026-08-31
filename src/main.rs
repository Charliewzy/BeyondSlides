use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use beyond_slides::{
    ChatCompletionsClient, ChatCompletionsConfig, DenseSlideScorer, HybridSlideScorer,
    LectureAnalysisConfig, LectureAnalysisSession, LecturePassages, LexicalSlideScorer, SlideDeck,
    Transcript, ValidatedAnalysis, ValidatedSources, WindowingConfig, rank_oral_additions,
    render_report,
};
use serde::{Serialize, de::DeserializeOwned};

const API_BASE_URL_ENV: &str = "BEYOND_SLIDES_API_BASE_URL";
const API_KEY_ENV: &str = "BEYOND_SLIDES_API_KEY";
const MODEL_ENV: &str = "BEYOND_SLIDES_MODEL";

const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;
const MAX_CONCURRENT_WINDOWS: usize = 4;
const CANARY_MAX_OUTPUT_TOKENS: u32 = 16_384;

#[tokio::main]
async fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("BeyondSlides: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: Vec<OsString>) -> Result<(), Box<dyn Error>> {
    match arguments.as_slice() {
        [command, transcript_path, slides_path, output_path]
            if command == OsStr::new("canary") =>
        {
            run_canary(transcript_path, slides_path, output_path).await
        }
        [transcript_path, slides_path, annotations_path, report_path] => run_report(
            transcript_path,
            slides_path,
            annotations_path,
            report_path,
        ),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: beyond-slides <transcript.json> <slides.json> <annotations.json> <result.html>\n\
             or:    beyond-slides canary <transcript.json> <slides.json> <canary.json>\n\
             canary requires BEYOND_SLIDES_API_BASE_URL, BEYOND_SLIDES_API_KEY, and \
             BEYOND_SLIDES_MODEL",
        )
        .into()),
    }
}

fn run_report(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    annotations_path: &OsStr,
    report_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let annotations_path = PathBuf::from(annotations_path);
    let report_path = PathBuf::from(report_path);

    let transcript: Transcript = read_json(&transcript_path, "transcript")?;
    let slide_deck: SlideDeck = read_json(&slides_path, "slides")?;
    let passages: LecturePassages = read_json(&annotations_path, "annotations")?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;
    let analysis = ValidatedAnalysis::new(sources, passages)?;
    let ranked = rank_oral_additions(&analysis);
    let report = render_report(&analysis, &ranked);

    write_file(&report_path, report, "report")?;
    Ok(())
}

async fn run_canary(
    transcript_path: &OsStr,
    slides_path: &OsStr,
    output_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let output_path = PathBuf::from(output_path);
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

    let client = ChatCompletionsClient::new(
        ChatCompletionsConfig::new(
            required_environment_variable(API_BASE_URL_ENV)?,
            required_environment_variable(API_KEY_ENV)?,
            required_environment_variable(MODEL_ENV)?,
        )?
        .with_max_output_tokens(CANARY_MAX_OUTPUT_TOKENS)?,
    );
    let windowing = WindowingConfig::new(
        MAX_OWNED_CHARACTERS,
        Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
        CONTEXT_CHARACTERS,
    )?;
    let config = LectureAnalysisConfig::new(windowing, MAX_CONCURRENT_WINDOWS)?;

    eprintln!("Preparing the lecture and scoring every transcript window...");
    let mut session = LectureAnalysisSession::prepare(&client, sources, &hybrid, config)?;
    eprintln!("Sending transcript window 1 as the canary...");
    let canary = session.analyze_canary().await?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot run a canary for an empty transcript",
        )
    })?;

    write_json(&output_path, canary, "canary analysis")?;
    println!(
        "Wrote {} canary passages to {} ({} tool rounds, prompt tokens: {}, completion tokens: {})",
        canary.analysis.passages.len(),
        output_path.display(),
        canary.diagnostics.tool_rounds,
        display_token_count(canary.diagnostics.prompt_tokens),
        display_token_count(canary.diagnostics.completion_tokens),
    );
    Ok(())
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
    let json = fs::read_to_string(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {kind} {}: {error}", path.display()),
        )
    })?;
    serde_json::from_str(&json).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {kind} JSON in {}: {error}", path.display()),
        )
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T, kind: &str) -> Result<(), io::Error> {
    let mut json = serde_json::to_string_pretty(value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not serialize {kind}: {error}"),
        )
    })?;
    json.push('\n');
    write_file(path, json, kind)
}

fn write_file(path: &Path, contents: String, kind: &str) -> Result<(), io::Error> {
    fs::write(path, contents).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {kind} {}: {error}", path.display()),
        )
    })
}
