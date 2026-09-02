mod analysis_run;
mod restoration_run;
mod run_support;
mod trace_summary_run;

use std::{
    env,
    error::Error,
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use beyond_slides::{
    LecturePassages, SlideDeck, Transcript, ValidatedAnalysis, ValidatedSources,
    rank_oral_additions, render_report,
};
use serde::de::DeserializeOwned;

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
        [command, trace_path] if command == OsStr::new("summarize-trace") => {
            trace_summary_run::run(trace_path)
        }
        [command, transcript_path, run_directory]
            if command == OsStr::new("restore-canary") =>
        {
            restoration_run::run_canary(transcript_path, run_directory).await
        }
        [command, transcript_path, run_directory] if command == OsStr::new("restore") => {
            restoration_run::run_complete(transcript_path, run_directory).await
        }
        [command, transcript_path, slides_path, output_path]
            if command == OsStr::new("canary") =>
        {
            analysis_run::run_canary(transcript_path, slides_path, output_path).await
        }
        [command, transcript_path, slides_path, run_directory]
            if command == OsStr::new("analyze") =>
        {
            analysis_run::run_complete(transcript_path, slides_path, run_directory).await
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
             or:    beyond-slides summarize-trace <model-trace.jsonl>\n\
             or:    beyond-slides canary <transcript.json> <slides.json> <canary.json>\n\
             or:    beyond-slides analyze <transcript.json> <slides.json> <run-directory>\n\
             or:    beyond-slides restore-canary <transcript.json> <run-directory>\n\
             or:    beyond-slides restore <transcript.json> <run-directory>\n\
             model-backed commands require BEYOND_SLIDES_API_BASE_URL, \
             BEYOND_SLIDES_API_KEY, and BEYOND_SLIDES_MODEL; optional \
             BEYOND_SLIDES_CHAT_EXTRA_BODY contains provider-specific JSON",
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

    fs::write(&report_path, report).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write report {}: {error}", report_path.display()),
        )
    })?;
    Ok(())
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
