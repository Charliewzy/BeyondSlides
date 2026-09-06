use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SlideDeck, SlideId, SlideScore,
    SlideScorer, Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources,
    WindowingConfig, build_windows, infer_slide_positions,
};
use serde::{Serialize, de::DeserializeOwned};

const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;

#[derive(Serialize)]
struct SemanticAlignmentProbe {
    transcript_segment_count: usize,
    slide_count: usize,
    scoring_mode: &'static str,
    dense_model: &'static str,
    alignment_algorithm: &'static str,
    windows: Vec<WindowAlignment>,
}

#[derive(Serialize)]
struct WindowAlignment {
    number: usize,
    owned_region: SentenceRange,
    query: String,
    slide_scores: Vec<SlideScore>,
    slide_position: SlideId,
}

#[derive(Serialize)]
struct SentenceRange {
    first_segment: TranscriptSegmentId,
    last_segment: TranscriptSegmentId,
    start_ms: u64,
    end_ms: u64,
    segment_count: usize,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [transcript_path, slides_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --release --example probe_semantic_alignment -- \
             <transcript.json> <slides.json> <alignment.json>",
        )
        .into());
    };

    let transcript: Transcript = read_json(Path::new(transcript_path))?;
    let slide_deck: SlideDeck = read_json(Path::new(slides_path))?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;
    if !sources.transcript().has_timestamps() {
        return Err("The time-based alignment probe requires a timestamped transcript".into());
    }
    let windows = build_windows(
        &sources,
        WindowingConfig::new(
            MAX_OWNED_CHARACTERS,
            Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
            CONTEXT_CHARACTERS,
        )?,
    );

    eprintln!(
        "Loading BAAI/bge-small-zh-v1.5 and scoring {} windows against {} slides...",
        windows.len(),
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);

    let mut queries = Vec::with_capacity(windows.len());
    let mut score_rows = Vec::with_capacity(windows.len());
    for (position, window) in windows.iter().enumerate() {
        let query = window
            .owned_region()
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        score_rows.push(hybrid.score_slides(&query)?);
        queries.push(query);
        if (position + 1) % 20 == 0 || position + 1 == windows.len() {
            eprintln!(
                "Scored {}/{} transcript windows",
                position + 1,
                windows.len()
            );
        }
    }

    let slide_positions = infer_slide_positions(&score_rows)?;
    let window_alignments = windows
        .iter()
        .zip(queries)
        .zip(score_rows)
        .zip(slide_positions)
        .enumerate()
        .map(
            |(position, (((window, query), slide_scores), slide_position))| {
                let owned_region = segment_range(window.owned_region()).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "window has an empty owned region",
                    )
                })?;
                Ok(WindowAlignment {
                    number: position + 1,
                    owned_region,
                    query,
                    slide_scores,
                    slide_position,
                })
            },
        )
        .collect::<Result<Vec<_>, io::Error>>()?;

    let probe = SemanticAlignmentProbe {
        transcript_segment_count: sources.transcript().segments.len(),
        slide_count: sources.slide_deck().slides.len(),
        scoring_mode: "hybrid",
        dense_model: "BAAI/bge-small-zh-v1.5",
        alignment_algorithm: "row-min-max-soft-non-monotonic-dp-v1",
        windows: window_alignments,
    };
    let output_path = PathBuf::from(output_path);
    let mut json = serde_json::to_string_pretty(&probe)?;
    json.push('\n');
    fs::write(&output_path, json).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", output_path.display()),
        )
    })?;
    println!(
        "Wrote {} inferred slide positions to {}",
        probe.windows.len(),
        output_path.display()
    );
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", path.display()),
        )
    })?;
    serde_json::from_str(&contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not parse {}: {error}", path.display()),
        )
        .into()
    })
}

fn segment_range(segments: &[TranscriptSegment]) -> Option<SentenceRange> {
    let first = segments.first()?;
    let last = segments.last()?;
    Some(SentenceRange {
        first_segment: first.id,
        last_segment: last.id,
        start_ms: first.start_ms?,
        end_ms: last.end_ms?,
        segment_count: segments.len(),
    })
}
