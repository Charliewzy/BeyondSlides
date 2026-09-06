use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use beyond_slides::{
    DenseSlideScorer, HybridSlideScorer, LexicalSlideScorer, SlideDeck, SlideId, SlideScorer,
    Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources, WindowingConfig,
    build_windows,
};
use serde::{Serialize, de::DeserializeOwned};

const MAX_OWNED_CHARACTERS: usize = 400;
const MAX_OWNED_DURATION_SECONDS: u64 = 60;
const CONTEXT_CHARACTERS: usize = 150;
const MAX_RESULTS: usize = 5;

#[derive(Serialize)]
struct RetrievalProbe {
    transcript_segment_count: usize,
    slide_count: usize,
    windowing: WindowingSettings,
    retrieval: RetrievalSettings,
    windows: Vec<WindowResult>,
}

#[derive(Serialize)]
struct WindowingSettings {
    max_owned_characters: usize,
    max_owned_duration_seconds: u64,
    context_characters: usize,
}

#[derive(Serialize)]
struct RetrievalSettings {
    mode: &'static str,
    dense_model: &'static str,
    max_results: usize,
}

#[derive(Serialize)]
struct WindowResult {
    number: usize,
    left_context: Option<SentenceRange>,
    owned_region: SentenceRange,
    right_context: Option<SentenceRange>,
    query: String,
    candidates: Vec<SlideCandidate>,
}

#[derive(Serialize)]
struct SentenceRange {
    first_segment: TranscriptSegmentId,
    last_segment: TranscriptSegmentId,
    start_ms: u64,
    end_ms: u64,
    segment_count: usize,
}

#[derive(Serialize)]
struct SlideCandidate {
    slide_id: SlideId,
    score: f64,
    text: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [transcript_path, slides_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --release --example probe_retrieval -- \
             <transcript.json> <slides.json> <output.json>",
        )
        .into());
    };
    let transcript_path = PathBuf::from(transcript_path);
    let slides_path = PathBuf::from(slides_path);
    let output_path = PathBuf::from(output_path);

    let transcript: Transcript = read_json(&transcript_path)?;
    let slide_deck: SlideDeck = read_json(&slides_path)?;
    let sources = ValidatedSources::new(transcript, slide_deck)?;
    if !sources.transcript().has_timestamps() {
        return Err("The time-based retrieval probe requires a timestamped transcript".into());
    }
    let windowing = WindowingConfig::new(
        MAX_OWNED_CHARACTERS,
        Duration::from_secs(MAX_OWNED_DURATION_SECONDS),
        CONTEXT_CHARACTERS,
    )?;
    let windows = build_windows(&sources, windowing);

    eprintln!(
        "Loading BAAI/bge-small-zh-v1.5 and indexing {} slides...",
        sources.slide_deck().slides.len()
    );
    let lexical = LexicalSlideScorer::new(&sources);
    let dense = DenseSlideScorer::try_new(&sources)?;
    let hybrid = HybridSlideScorer::new(&lexical, &dense);

    let mut results = Vec::with_capacity(windows.len());
    for (position, window) in windows.iter().enumerate() {
        let query = window
            .owned_region()
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut scores = hybrid.score_slides(&query)?;
        scores.sort_by(|left, right| right.score.total_cmp(&left.score));
        scores.truncate(MAX_RESULTS);
        let candidates = scores
            .into_iter()
            .map(|hit| {
                let slide = sources.slide_deck().find(hit.slide_id).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("retrieval returned unknown slide {}", hit.slide_id.0),
                    )
                })?;
                Ok(SlideCandidate {
                    slide_id: hit.slide_id,
                    score: hit.score,
                    text: slide.text.clone(),
                })
            })
            .collect::<Result<Vec<_>, io::Error>>()?;

        results.push(WindowResult {
            number: position + 1,
            left_context: segment_range(window.left_context()),
            owned_region: segment_range(window.owned_region()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "window has an empty owned region",
                )
            })?,
            right_context: segment_range(window.right_context()),
            query,
            candidates,
        });

        if (position + 1) % 20 == 0 || position + 1 == windows.len() {
            eprintln!(
                "Searched {}/{} transcript windows",
                position + 1,
                windows.len()
            );
        }
    }

    let probe = RetrievalProbe {
        transcript_segment_count: sources.transcript().segments.len(),
        slide_count: sources.slide_deck().slides.len(),
        windowing: WindowingSettings {
            max_owned_characters: MAX_OWNED_CHARACTERS,
            max_owned_duration_seconds: MAX_OWNED_DURATION_SECONDS,
            context_characters: CONTEXT_CHARACTERS,
        },
        retrieval: RetrievalSettings {
            mode: "hybrid",
            dense_model: "BAAI/bge-small-zh-v1.5",
            max_results: MAX_RESULTS,
        },
        windows: results,
    };
    let mut json = serde_json::to_string_pretty(&probe)?;
    json.push('\n');
    fs::write(&output_path, json).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", output_path.display()),
        )
    })?;
    println!(
        "Wrote {} window results to {}",
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
