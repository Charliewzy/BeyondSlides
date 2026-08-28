use std::{
    env,
    error::Error,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use beyond_slides::{
    LecturePassages, SlideDeck, Transcript, ValidatedAnalysis, ValidatedSources,
    rank_oral_additions, render_report,
};
use serde::de::DeserializeOwned;

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("BeyondSlides: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<impl AsRef<OsStr>>) -> Result<(), Box<dyn Error>> {
    let [transcript_path, slides_path, annotations_path, report_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: beyond-slides <transcript.json> <slides.json> <annotations.json> <result.html>",
        )
        .into());
    };
    let transcript_path = PathBuf::from(transcript_path.as_ref());
    let slides_path = PathBuf::from(slides_path.as_ref());
    let annotations_path = PathBuf::from(annotations_path.as_ref());
    let report_path = PathBuf::from(report_path.as_ref());

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
