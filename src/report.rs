use askama::Template;

use crate::{LecturePassage, Slide, TranscriptSentence, ValidatedAnalysis};

pub fn render_report(analysis: &ValidatedAnalysis, ranked: &[&LecturePassage]) -> String {
    ReportTemplate {
        ranked: ranked
            .iter()
            .filter_map(|passage| PassageView::new(analysis, passage))
            .collect(),
        transcript: analysis
            .passages()
            .iter()
            .filter_map(|passage| PassageView::new(analysis, passage))
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
    connection_strength: u8,
    transcript: String,
    sentences: Vec<&'a str>,
    related_slides: Vec<SlideView<'a>>,
}

impl<'a> PassageView<'a> {
    fn new(analysis: &'a ValidatedAnalysis, passage: &'a LecturePassage) -> Option<Self> {
        let sentences: Vec<_> = passage_sentences(analysis, passage).collect();
        let (Some(first), Some(last)) = (sentences.first(), sentences.last()) else {
            return None;
        };
        let transcript = sentences
            .iter()
            .map(|sentence| sentence.text.as_str())
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
                analysis
                    .slide_deck()
                    .find(related_slide)
                    .map(SlideView::from)
            })
            .collect::<Option<Vec<_>>>()?;

        Some(Self {
            start: passage.start.0,
            timestamp: format!(
                "{}–{}",
                format_timestamp(first.start_ms),
                format_timestamp(last.end_ms)
            ),
            title,
            importance: passage.importance.get(),
            novelty: passage.novelty.get(),
            connection_strength: passage.connection_strength.get(),
            transcript,
            sentences: sentences
                .into_iter()
                .map(|sentence| sentence.text.as_str())
                .collect(),
            related_slides,
        })
    }
}

struct SlideView<'a> {
    id: u32,
    text: &'a str,
}

impl<'a> From<&'a Slide> for SlideView<'a> {
    fn from(slide: &'a Slide) -> Self {
        Self {
            id: slide.id.0,
            text: slide.text.as_str(),
        }
    }
}

fn passage_sentences<'a>(
    analysis: &'a ValidatedAnalysis,
    passage: &'a LecturePassage,
) -> impl Iterator<Item = &'a TranscriptSentence> {
    analysis
        .transcript()
        .sentences
        .iter()
        .skip_while(|sentence| sentence.id != passage.start)
        .scan(false, |finished, sentence| {
            if *finished {
                return None;
            }
            *finished = sentence.id == passage.end;
            Some(sentence)
        })
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
