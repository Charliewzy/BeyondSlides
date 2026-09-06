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
    if media.audio.is_some()
        && !analysis.transcript().has_timestamps()
        && media.playback_intervals.is_empty()
    {
        return Err(ContinuousReportError::MissingPlaybackTiming);
    }
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
        duration: analysis
            .transcript()
            .segments
            .last()
            .and_then(|segment| segment.end_ms)
            .map(format_timestamp)
            .unwrap_or_else(|| "未提供时间戳".into()),
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

struct PassageView {
    source_start: u32,
    source_end: u32,
    timestamp: String,
    audio_start_ms: String,
    audio_end_ms: String,
    audio_basis: &'static str,
    slide_position: u32,
    slide_number: usize,
    importance: u8,
    novelty: u8,
    importance_percentile: String,
    novelty_percentile: String,
    connection_strength: String,
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
        let coarse_playback =
            first
                .start_ms
                .zip(last.end_ms)
                .map(|(start_ms, end_ms)| PassagePlaybackInterval {
                    start_ms,
                    end_ms,
                    basis: crate::PlaybackTimingBasis::TranscriptSegments,
                });
        let playback = playback.or(coarse_playback.as_ref());
        Self {
            source_start: passage.source_start.0,
            source_end: passage.source_end.0,
            timestamp: playback
                .map(|playback| {
                    format!(
                        "{}–{}",
                        format_timestamp(playback.start_ms),
                        format_timestamp(playback.end_ms)
                    )
                })
                .unwrap_or_else(|| "未提供时间戳".into()),
            audio_start_ms: playback.map(|p| p.start_ms.to_string()).unwrap_or_default(),
            audio_end_ms: playback.map(|p| p.end_ms.to_string()).unwrap_or_default(),
            audio_basis: match playback.map(|p| p.basis) {
                Some(crate::PlaybackTimingBasis::TimedTokens) => "timed_tokens",
                Some(crate::PlaybackTimingBasis::TranscriptSegments) => "transcript_segments",
                None => "unavailable",
            },
            slide_position: passage.slide_position.0,
            slide_number: passage.slide_position.index() + 1,
            importance: passage.importance.get(),
            novelty: passage.novelty.get(),
            importance_percentile: passage
                .comparative_importance
                .map(|score| format!("{:.1}", score.percentile()))
                .unwrap_or_default(),
            novelty_percentile: passage
                .comparative_novelty
                .map(|score| format!("{:.1}", score.percentile()))
                .unwrap_or_default(),
            connection_strength: passage
                .connection_strength
                .map(|score| score.get().to_string())
                .unwrap_or_default(),
            text: passage.text.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousReportError {
    MissingPlaybackTiming,
    SlideImageCountMismatch { expected: usize, actual: usize },
    PlaybackIntervalCountMismatch { expected: usize, actual: usize },
}

impl fmt::Display for ContinuousReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPlaybackTiming => formatter.write_str("untimed transcripts need independently aligned playback intervals before enabling passage audio"),
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
