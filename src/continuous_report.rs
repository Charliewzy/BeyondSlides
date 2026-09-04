use std::{error::Error, fmt};

use askama::Template;

use crate::{PassagePlaybackInterval, RestoredLecturePassage, ValidatedRestoredAnalysis};

/// One report-local image corresponding to a slide in presentation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSlideImage {
    pub source: String,
    pub width: u32,
    pub height: u32,
}

/// One report-local recording used for passage-scoped playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportAudio {
    pub source: String,
}

/// Optional presentation assets used by the continuous lecture report.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ContinuousReportMedia {
    pub slide_images: Vec<ReportSlideImage>,
    pub audio: Option<ReportAudio>,
    pub playback_intervals: Vec<PassagePlaybackInterval>,
}

/// Renders the readable lecture in chronological order with inline score typography.
pub fn render_continuous_report(analysis: &ValidatedRestoredAnalysis) -> String {
    render_continuous_report_with_media(analysis, &ContinuousReportMedia::default())
        .expect("an empty media collection is always valid")
}

/// Renders the readable lecture together with optional report-local media.
pub fn render_continuous_report_with_media(
    analysis: &ValidatedRestoredAnalysis,
    media: &ContinuousReportMedia,
) -> Result<String, ContinuousReportError> {
    if !media.slide_images.is_empty()
        && media.slide_images.len() != analysis.slide_deck().slides.len()
    {
        return Err(ContinuousReportError::SlideImageCountMismatch {
            expected: analysis.slide_deck().slides.len(),
            actual: media.slide_images.len(),
        });
    }
    if !media.playback_intervals.is_empty()
        && media.playback_intervals.len() != analysis.passages().len()
    {
        return Err(ContinuousReportError::PlaybackIntervalCountMismatch {
            expected: analysis.passages().len(),
            actual: media.playback_intervals.len(),
        });
    }

    let passages: Vec<_> = analysis
        .passages()
        .iter()
        .enumerate()
        .map(|(index, passage)| {
            PassageView::new(analysis, passage, media.playback_intervals.get(index))
        })
        .collect();
    let slides: Vec<_> = media
        .slide_images
        .iter()
        .enumerate()
        .map(|(index, image)| SlideView {
            id: index,
            number: index + 1,
            source: image.source.as_str(),
            width: image.width,
            height: image.height,
        })
        .collect();
    Ok(ContinuousReportTemplate {
        passage_count: passages.len(),
        duration: analysis.transcript().segments.last().map_or_else(
            || "00:00".into(),
            |segment| format_timestamp(segment.end_ms),
        ),
        importance_distribution: score_distribution(analysis, |passage| passage.importance.get()),
        novelty_distribution: score_distribution(analysis, |passage| passage.novelty.get()),
        has_slides: !slides.is_empty(),
        has_audio: media.audio.is_some(),
        audio_source: media.audio.as_ref().map_or("", |audio| &audio.source),
        slides,
        passages,
    }
    .to_string())
}

#[derive(Template)]
#[template(path = "continuous_report.html")]
struct ContinuousReportTemplate<'a> {
    passage_count: usize,
    duration: String,
    importance_distribution: Vec<ScoreCount>,
    novelty_distribution: Vec<ScoreCount>,
    has_slides: bool,
    has_audio: bool,
    audio_source: &'a str,
    slides: Vec<SlideView<'a>>,
    passages: Vec<PassageView>,
}

struct SlideView<'a> {
    id: usize,
    number: usize,
    source: &'a str,
    width: u32,
    height: u32,
}

struct ScoreCount {
    score: usize,
    count: usize,
}

struct PassageView {
    source_start: u32,
    source_end: u32,
    timestamp: String,
    audio_start_ms: u64,
    audio_end_ms: u64,
    audio_basis: &'static str,
    slide_position: u32,
    slide_number: usize,
    importance: u8,
    novelty: u8,
    connection_strength: u8,
    text: String,
}

impl PassageView {
    fn new(
        analysis: &ValidatedRestoredAnalysis,
        passage: &RestoredLecturePassage,
        playback: Option<&PassagePlaybackInterval>,
    ) -> Self {
        let first = &analysis.transcript().segments[passage.source_start.index()];
        let last = &analysis.transcript().segments[passage.source_end.index()];
        let coarse_playback = PassagePlaybackInterval {
            start_ms: first.start_ms,
            end_ms: last.end_ms,
            basis: crate::PlaybackTimingBasis::TranscriptSegments,
        };
        let playback = playback.unwrap_or(&coarse_playback);
        Self {
            source_start: passage.source_start.0,
            source_end: passage.source_end.0,
            timestamp: format!(
                "{}–{}",
                format_timestamp(playback.start_ms),
                format_timestamp(playback.end_ms)
            ),
            audio_start_ms: playback.start_ms,
            audio_end_ms: playback.end_ms,
            audio_basis: match playback.basis {
                crate::PlaybackTimingBasis::TimedTokens => "timed_tokens",
                crate::PlaybackTimingBasis::TranscriptSegments => "transcript_segments",
            },
            slide_position: passage.slide_position.0,
            slide_number: passage.slide_position.index() + 1,
            importance: passage.importance.get(),
            novelty: passage.novelty.get(),
            connection_strength: passage.connection_strength.get(),
            text: passage.text.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousReportError {
    SlideImageCountMismatch { expected: usize, actual: usize },
    PlaybackIntervalCountMismatch { expected: usize, actual: usize },
}

impl fmt::Display for ContinuousReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlideImageCountMismatch { expected, actual } => write!(
                formatter,
                "continuous report expected {expected} slide images but received {actual}"
            ),
            Self::PlaybackIntervalCountMismatch { expected, actual } => write!(
                formatter,
                "continuous report expected {expected} passage playback intervals but received {actual}"
            ),
        }
    }
}

impl Error for ContinuousReportError {}

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
