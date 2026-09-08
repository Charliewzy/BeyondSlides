mod analysis_run;
mod boundary_run;
mod report_assets;
mod restoration_review_run;
mod restoration_run;
mod run_support;
mod trace_summary_run;
mod web;
mod worker_control;

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
        [command] if command == OsStr::new("serve") => web::serve(OsStr::new("run/application"), 7842).await,
        [command, directory, port] if command == OsStr::new("serve") => {
            let port: u16 = port.to_str().ok_or("invalid port")?.parse()?;
            web::serve(directory, port).await
        }
        [command, directory] if command == OsStr::new("application-worker") => web::worker::run(directory).await,
        [command, trace_path] if command == OsStr::new("summarize-trace") => {
            trace_summary_run::run(trace_path)
        }
        [command, transcript_path, run_directory]
            if command == OsStr::new("restore-canary") =>
        {
            restoration_run::run_canary(transcript_path, run_directory).await
        }
        [command, transcript_path, run_directory] if command == OsStr::new("restore") => {
            restoration_run::run_complete(transcript_path, run_directory, None).await
        }
        [command, transcript_path, run_directory, output_path]
            if command == OsStr::new("review-restoration") =>
        {
            restoration_review_run::run(transcript_path, run_directory, output_path)
        }
        [command, transcript_path, slides_path, output_path]
            if command == OsStr::new("canary") =>
        {
            analysis_run::run_canary(transcript_path, slides_path, output_path).await
        }
        [command, transcript_path, slides_path, run_directory, options @ ..]
            if command == OsStr::new("analyze") =>
        {
            let options = parse_report_options(options)?;
            analysis_run::run_complete(
                transcript_path,
                slides_path,
                run_directory,
                options.slide_pdf.as_deref(),
                options.audio.as_deref(),
                options.timed_tokens.as_deref(),
            )
            .await
        }
        [command, transcript_path, slides_path, analysis_path, report_path, options @ ..]
            if command == OsStr::new("render-analysis") =>
        {
            let options = parse_report_options(options)?;
            analysis_run::render_saved_analysis(
                transcript_path,
                slides_path,
                analysis_path,
                report_path,
                options.slide_pdf.as_deref(),
                options.audio.as_deref(),
                options.timed_tokens.as_deref(),
            )
        }
        [command, analysis_path, trace_path]
            if command == OsStr::new("evaluate-analysis") =>
        {
            analysis_run::evaluate_saved_analysis(analysis_path, trace_path)
        }
        [transcript_path, slides_path, annotations_path, report_path] => run_report(
            transcript_path,
            slides_path,
            annotations_path,
            report_path,
        ),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: beyond-slides serve [<application-directory> <port>]\n\
             or:    beyond-slides <transcript.json> <slides.json> <annotations.json> <result.html>\n\
             or:    beyond-slides summarize-trace <model-trace.jsonl>\n\
             or:    beyond-slides canary <transcript.json> <slides.json> <canary.json>\n\
             or:    beyond-slides analyze <transcript.json> <slides.json> <run-directory> [--slides-pdf <slides.pdf>] [--audio <recording> --timed-tokens <timing.json>]\n\
             or:    beyond-slides render-analysis <transcript.json> <slides.json> <analysis.json> <result.html> [--slides-pdf <slides.pdf>] [--audio <recording> --timed-tokens <timing.json>]\n\
             or:    beyond-slides evaluate-analysis <analysis.json> <model-trace.jsonl>\n\
             or:    beyond-slides restore-canary <transcript.json> <run-directory>\n\
             or:    beyond-slides restore <transcript.json> <run-directory>\n\
             or:    beyond-slides review-restoration <transcript.json> <run-directory> <result.html>\n\
             model-backed commands require BEYOND_SLIDES_API_BASE_URL, \
             BEYOND_SLIDES_API_KEY, and BEYOND_SLIDES_MODEL; optional \
             BEYOND_SLIDES_CHAT_EXTRA_BODY contains provider-specific JSON; optional \n\
             BEYOND_SLIDES_RESTORATION_CHAT_EXTRA_BODY and \n\
             BEYOND_SLIDES_ANNOTATION_CHAT_EXTRA_BODY override it per stage; Codex also accepts \
             BEYOND_SLIDES_CODEX_REASONING_EFFORT and BEYOND_SLIDES_CODEX_SERVICE_TIER",
        )
        .into()),
    }
}

#[derive(Default)]
struct ReportOptions {
    slide_pdf: Option<OsString>,
    audio: Option<OsString>,
    timed_tokens: Option<OsString>,
}

fn parse_report_options(arguments: &[OsString]) -> Result<ReportOptions, io::Error> {
    let mut options = ReportOptions::default();
    let mut arguments = arguments.iter();
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("missing path after {}", flag.to_string_lossy()),
            )
        })?;
        let destination = match flag.as_os_str() {
            flag if flag == OsStr::new("--slides-pdf") => &mut options.slide_pdf,
            flag if flag == OsStr::new("--audio") => &mut options.audio,
            flag if flag == OsStr::new("--timed-tokens") => &mut options.timed_tokens,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown report option {}", flag.to_string_lossy()),
                ));
            }
        };
        if destination.replace(value.clone()).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate report option {}", flag.to_string_lossy()),
            ));
        }
    }
    Ok(options)
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
