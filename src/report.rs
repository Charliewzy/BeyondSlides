use askama::Template;

use crate::{LecturePassage, Slide, TranscriptSegment, ValidatedAnalysis};

pub fn render_report(analysis: &ValidatedAnalysis, ranked: &[&LecturePassage]) -> String {
    ReportTemplate {
        ranked: ranked
            .iter()
            .map(|passage| PassageView::new(analysis, passage))
            .collect(),
        transcript: analysis
            .passages()
            .iter()
            .map(|passage| PassageView::new(analysis, passage))
            .collect(),
    }
    .to_string()
}

#[derive(Template)]
#[template(path = "report.html")]
struct ReportTemplate<'a> {
    ranked: Vec<PassageView<'a>>,
    transcript: Vec<PassageView<'a>>,
}

struct PassageView<'a> {
    start: u32,
    timestamp: String,
    title: String,
    importance: u8,
    novelty: u8,
    transcript: String,
    segments: Vec<&'a str>,
    related_slides: Vec<SlideView<'a>>,
}

impl<'a> PassageView<'a> {
    fn new(analysis: &'a ValidatedAnalysis, passage: &'a LecturePassage) -> Self {
        let segments = passage_segments(analysis, passage);
        let first = &segments[0];
        let last = &segments[segments.len() - 1];
        let transcript = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let title = passage
            .summary
            .as_deref()
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or(&transcript)
            .to_owned();
        let related_slides = passage
            .related_slides
            .iter()
            .map(|&related_slide| {
                SlideView::from(&analysis.slide_deck().slides[related_slide.index()])
            })
            .collect();

        Self {
            start: passage.start.0,
            timestamp: format!(
                "{}–{}",
                format_timestamp(first.start_ms),
                format_timestamp(last.end_ms)
            ),
            title,
            importance: passage.importance.get(),
            novelty: passage.novelty.get(),
            transcript,
            segments: segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect(),
            related_slides,
        }
    }
}

struct SlideView<'a> {
    number: u64,
    text: &'a str,
}

impl<'a> From<&'a Slide> for SlideView<'a> {
    fn from(slide: &'a Slide) -> Self {
        Self {
            number: u64::from(slide.id.0) + 1,
            text: slide.text.as_str(),
        }
    }
}

fn passage_segments<'a>(
    analysis: &'a ValidatedAnalysis,
    passage: &'a LecturePassage,
) -> &'a [TranscriptSegment] {
    &analysis.transcript().segments[passage.start.index()..=passage.end.index()]
}

fn format_timestamp(milliseconds: Option<u64>) -> String {
    let Some(milliseconds) = milliseconds else {
        return "无时间戳".into();
    };
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
