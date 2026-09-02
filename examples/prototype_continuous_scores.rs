//! THROWAWAY UI PROTOTYPE: continuous transcript score typography.
//!
//! Run with:
//! cargo run --example prototype_continuous_scores -- \
//!   <transcript.json> <slides.json> <analysis.json> <output.html>

use std::{env, error::Error, ffi::OsString, fs, io, path::Path};

use askama::Template;
use beyond_slides::{
    LecturePassage, LecturePassages, SlideDeck, Transcript, ValidatedAnalysis, ValidatedSources,
};
use serde::Deserialize;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [transcript_path, slides_path, analysis_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: prototype_continuous_scores <transcript.json> <slides.json> <analysis.json> <output.html>",
        )
        .into());
    };
    let analysis = load_analysis(transcript_path, slides_path, analysis_path)?;
    let html = render(&analysis);
    fs::write(output_path, html)?;
    println!(
        "Wrote throwaway continuous-score prototype to {}",
        Path::new(output_path).display()
    );
    Ok(())
}

fn load_analysis(
    transcript_path: &OsString,
    slides_path: &OsString,
    analysis_path: &OsString,
) -> Result<ValidatedAnalysis, Box<dyn Error>> {
    let transcript: Transcript = read_json(transcript_path)?;
    let slides: SlideDeck = read_json(slides_path)?;
    let annotations: AnalysisInput = read_json(analysis_path)?;
    let sources = ValidatedSources::new(transcript, slides)?;
    Ok(ValidatedAnalysis::new(
        sources,
        LecturePassages {
            passages: annotations.passages,
        },
    )?)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &OsString) -> Result<T, Box<dyn Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn render(analysis: &ValidatedAnalysis) -> String {
    let passages = analysis
        .passages()
        .iter()
        .map(|passage| passage_view(analysis, passage))
        .collect::<Vec<_>>();
    ContinuousScoresPrototype {
        passage_count: passages.len(),
        duration: analysis.transcript().segments.last().map_or_else(
            || "00:00".into(),
            |segment| format_timestamp(segment.end_ms),
        ),
        importance_distribution: score_distribution(analysis, |passage| passage.importance.get()),
        novelty_distribution: score_distribution(analysis, |passage| passage.novelty.get()),
        passages,
    }
    .to_string()
}

fn passage_view(analysis: &ValidatedAnalysis, passage: &LecturePassage) -> PassageView {
    let segments = &analysis.transcript().segments[passage.start.index()..=passage.end.index()];
    let first = &segments[0];
    let last = &segments[segments.len() - 1];
    let importance = passage.importance.get();
    let novelty = passage.novelty.get();
    PassageView {
        start: passage.start.0,
        end: passage.end.0,
        timestamp: format!(
            "{}–{}",
            format_timestamp(first.start_ms),
            format_timestamp(last.end_ms)
        ),
        importance,
        novelty,
        connection_strength: passage.connection_strength.get(),
        text: segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect(),
        editorial_weight: weight(importance, 360, 105),
        signal_weight: weight(importance, 300, 120),
        manuscript_weight: weight(importance, 350, 90),
        editorial_underline: underline(novelty, 0.45, 0.7),
        signal_underline: underline(novelty, 0.65, 1.0),
        manuscript_underline: underline(novelty, 0.4, 0.55),
    }
}

fn score_distribution(
    analysis: &ValidatedAnalysis,
    score: impl Fn(&LecturePassage) -> u8,
) -> Vec<ScoreCount> {
    let mut counts = [0; 6];
    for passage in analysis.passages() {
        counts[usize::from(score(passage))] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(score, count)| ScoreCount { score, count })
        .collect()
}

fn weight(score: u8, base: u16, step: u16) -> u16 {
    base + u16::from(score) * step
}

fn underline(score: u8, base: f32, step: f32) -> String {
    format!("{:.2}", base + f32::from(score) * step)
}

fn format_timestamp(milliseconds: u64) -> String {
    let total_seconds = milliseconds / 1_000;
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

#[derive(Deserialize)]
struct AnalysisInput {
    passages: Vec<LecturePassage>,
}

#[derive(Template)]
#[template(path = "prototype_continuous_scores.html")]
struct ContinuousScoresPrototype {
    passage_count: usize,
    duration: String,
    importance_distribution: Vec<ScoreCount>,
    novelty_distribution: Vec<ScoreCount>,
    passages: Vec<PassageView>,
}

struct ScoreCount {
    score: usize,
    count: usize,
}

struct PassageView {
    start: u32,
    end: u32,
    timestamp: String,
    importance: u8,
    novelty: u8,
    connection_strength: u8,
    text: String,
    editorial_weight: u16,
    signal_weight: u16,
    manuscript_weight: u16,
    editorial_underline: String,
    signal_underline: String,
    manuscript_underline: String,
}
