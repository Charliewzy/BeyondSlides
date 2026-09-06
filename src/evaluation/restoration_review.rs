use std::{error::Error, fmt};

use askama::Template;

use crate::{
    RestoredTranscriptSpan, Transcript, TranscriptSegment, TranscriptWindowRestorationResult,
};

pub fn render_restoration_review(
    transcript: &Transcript,
    window_results: &[TranscriptWindowRestorationResult],
) -> Result<String, RestorationReviewError> {
    validate_transcript_ids(transcript)?;
    let mut next_source = 0;
    let mut windows = Vec::with_capacity(window_results.len());
    let mut restored_character_count = 0;
    let mut omitted_segment_count = 0;
    let mut repaired_window_count = 0;

    for (window_index, result) in window_results.iter().enumerate() {
        if result.restoration.spans.is_empty() {
            return Err(RestorationReviewError::EmptyWindow { window_index });
        }
        let mut groups = Vec::with_capacity(result.restoration.spans.len());
        let mut has_omission = false;
        let window_start = next_source;
        for span in &result.restoration.spans {
            let start = span.source_start().index();
            let end = span.source_end().index();
            if end < start {
                return Err(RestorationReviewError::ReversedRange {
                    window_index,
                    start,
                    end,
                });
            }
            if start != next_source {
                return Err(RestorationReviewError::CoverageMismatch {
                    window_index,
                    expected: next_source,
                    actual: start,
                });
            }
            let Some(source_segments) = transcript.segments.get(start..=end) else {
                return Err(RestorationReviewError::RangeOutsideTranscript {
                    window_index,
                    start,
                    end,
                    segment_count: transcript.segments.len(),
                });
            };
            let (kind, output, omitted) = match span {
                RestoredTranscriptSpan::Text { text, .. } => {
                    restored_character_count += text.chars().count();
                    ("恢复文本", text.clone(), false)
                }
                RestoredTranscriptSpan::OmittedDisfluency { .. } => {
                    has_omission = true;
                    omitted_segment_count += source_segments.len();
                    ("省略语气词", "此范围被明确标记为纯语气词。".into(), true)
                }
            };
            groups.push(SpanReviewView {
                range: source_range(start, end),
                source_segments: source_segments
                    .iter()
                    .map(SourceSegmentView::from)
                    .collect(),
                kind,
                output,
                omitted,
            });
            next_source = end + 1;
        }
        let window_end = next_source - 1;
        let first = &transcript.segments[window_start];
        let last = &transcript.segments[window_end];
        let has_repair = result.diagnostics.final_answer_repairs > 0;
        repaired_window_count += usize::from(has_repair);
        windows.push(WindowReviewView {
            index: window_index,
            number: window_index + 1,
            source_range: source_range(window_start, window_end),
            timestamp: format!(
                "{}–{}",
                format_timestamp(first.start_ms),
                format_timestamp(last.end_ms)
            ),
            provider_retries: result.diagnostics.provider_retries,
            repairs: result.diagnostics.final_answer_repairs,
            prompt_tokens: token_count(result.diagnostics.prompt_tokens),
            completion_tokens: token_count(result.diagnostics.completion_tokens),
            has_repair,
            has_omission,
            groups,
        });
    }

    if next_source != transcript.segments.len() {
        return Err(RestorationReviewError::UncoveredTranscriptTail {
            expected: next_source,
            segment_count: transcript.segments.len(),
        });
    }

    Ok(RestorationReviewTemplate {
        window_count: windows.len(),
        segment_count: transcript.segments.len(),
        repaired_window_count,
        omitted_segment_count,
        raw_character_count: transcript
            .segments
            .iter()
            .map(|segment| segment.text.chars().count())
            .sum(),
        restored_character_count,
        windows,
    }
    .to_string())
}

#[derive(Template)]
#[template(path = "restoration_review.html")]
struct RestorationReviewTemplate {
    window_count: usize,
    segment_count: usize,
    repaired_window_count: usize,
    omitted_segment_count: usize,
    raw_character_count: usize,
    restored_character_count: usize,
    windows: Vec<WindowReviewView>,
}

struct WindowReviewView {
    index: usize,
    number: usize,
    source_range: String,
    timestamp: String,
    provider_retries: usize,
    repairs: usize,
    prompt_tokens: String,
    completion_tokens: String,
    has_repair: bool,
    has_omission: bool,
    groups: Vec<SpanReviewView>,
}

struct SpanReviewView {
    range: String,
    source_segments: Vec<SourceSegmentView>,
    kind: &'static str,
    output: String,
    omitted: bool,
}

struct SourceSegmentView {
    id: u32,
    timestamp: String,
    text: String,
}

impl From<&TranscriptSegment> for SourceSegmentView {
    fn from(segment: &TranscriptSegment) -> Self {
        Self {
            id: segment.id.0,
            timestamp: format!(
                "{}–{}",
                format_timestamp(segment.start_ms),
                format_timestamp(segment.end_ms)
            ),
            text: segment.text.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorationReviewError {
    NonCanonicalSegmentId {
        position: usize,
        actual: u32,
    },
    EmptyWindow {
        window_index: usize,
    },
    ReversedRange {
        window_index: usize,
        start: usize,
        end: usize,
    },
    CoverageMismatch {
        window_index: usize,
        expected: usize,
        actual: usize,
    },
    RangeOutsideTranscript {
        window_index: usize,
        start: usize,
        end: usize,
        segment_count: usize,
    },
    UncoveredTranscriptTail {
        expected: usize,
        segment_count: usize,
    },
}

impl fmt::Display for RestorationReviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalSegmentId { position, actual } => write!(
                formatter,
                "transcript segment at position {position} has ID {actual}; expected {position}"
            ),
            Self::EmptyWindow { window_index } => write!(
                formatter,
                "restoration window index {window_index} has no restored spans"
            ),
            Self::ReversedRange {
                window_index,
                start,
                end,
            } => write!(
                formatter,
                "restoration window index {window_index} has a reversed source range {start}–{end}"
            ),
            Self::CoverageMismatch {
                window_index,
                expected,
                actual,
            } => write!(
                formatter,
                "restoration window index {window_index} expected source segment {expected} but found {actual}"
            ),
            Self::RangeOutsideTranscript {
                window_index,
                start,
                end,
                segment_count,
            } => write!(
                formatter,
                "restoration window index {window_index} references source range {start}–{end}, outside a transcript with {segment_count} segments"
            ),
            Self::UncoveredTranscriptTail {
                expected,
                segment_count,
            } => write!(
                formatter,
                "restoration results stop before source segment {expected}; the transcript has {segment_count} segments"
            ),
        }
    }
}

impl Error for RestorationReviewError {}

fn validate_transcript_ids(transcript: &Transcript) -> Result<(), RestorationReviewError> {
    for (position, segment) in transcript.segments.iter().enumerate() {
        if segment.id.index() != position {
            return Err(RestorationReviewError::NonCanonicalSegmentId {
                position,
                actual: segment.id.0,
            });
        }
    }
    Ok(())
}

fn source_range(start: usize, end: usize) -> String {
    if start == end {
        format!("#{start}")
    } else {
        format!("#{start}–{end}")
    }
}

fn token_count(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "未知".into(), |tokens| tokens.to_string())
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
