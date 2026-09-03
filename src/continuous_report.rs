use askama::Template;

use crate::{RestoredLecturePassage, ValidatedRestoredAnalysis};

/// Renders the readable lecture in chronological order with inline score typography.
pub fn render_continuous_report(analysis: &ValidatedRestoredAnalysis) -> String {
    let passages: Vec<_> = analysis
        .passages()
        .iter()
        .map(|passage| PassageView::new(analysis, passage))
        .collect();
    ContinuousReportTemplate {
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

#[derive(Template)]
#[template(path = "continuous_report.html")]
struct ContinuousReportTemplate {
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
    source_start: u32,
    source_end: u32,
    timestamp: String,
    importance: u8,
    novelty: u8,
    connection_strength: u8,
    text: String,
}

impl PassageView {
    fn new(analysis: &ValidatedRestoredAnalysis, passage: &RestoredLecturePassage) -> Self {
        let first = &analysis.transcript().segments[passage.source_start.index()];
        let last = &analysis.transcript().segments[passage.source_end.index()];
        Self {
            source_start: passage.source_start.0,
            source_end: passage.source_end.0,
            timestamp: format!(
                "{}–{}",
                format_timestamp(first.start_ms),
                format_timestamp(last.end_ms)
            ),
            importance: passage.importance.get(),
            novelty: passage.novelty.get(),
            connection_strength: passage.connection_strength.get(),
            text: passage.text.clone(),
        }
    }
}

fn score_distribution(
    analysis: &ValidatedRestoredAnalysis,
    score: impl Fn(&RestoredLecturePassage) -> u8,
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
