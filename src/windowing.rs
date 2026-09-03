use std::{error::Error, fmt, num::NonZeroUsize, time::Duration};

use crate::{
    RestoredTranscript, RestoredTranscriptSpan, TranscriptSegment, TranscriptSegmentId,
    ValidatedSources,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowingConfig {
    max_owned_characters: NonZeroUsize,
    max_owned_duration: Duration,
    context_characters: usize,
}

impl WindowingConfig {
    /// Configures complete-segment windows using Unicode character and
    /// wall-clock budgets. An individually oversized segment remains intact.
    pub fn new(
        max_owned_characters: usize,
        max_owned_duration: Duration,
        context_characters: usize,
    ) -> Result<Self, WindowingConfigError> {
        let Some(max_owned_characters) = NonZeroUsize::new(max_owned_characters) else {
            return Err(WindowingConfigError::EmptyCharacterBudget);
        };
        if max_owned_duration.is_zero() {
            return Err(WindowingConfigError::EmptyDurationBudget);
        }
        Ok(Self {
            max_owned_characters,
            max_owned_duration,
            context_characters,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowingConfigError {
    EmptyCharacterBudget,
    EmptyDurationBudget,
}

impl fmt::Display for WindowingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCharacterBudget => write!(
                formatter,
                "an owned region must allow at least one text character"
            ),
            Self::EmptyDurationBudget => {
                write!(formatter, "an owned region must allow a nonzero duration")
            }
        }
    }
}

impl Error for WindowingConfigError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptWindow<'a> {
    left_context: &'a [TranscriptSegment],
    owned_region: &'a [TranscriptSegment],
    right_context: &'a [TranscriptSegment],
}

impl<'a> TranscriptWindow<'a> {
    pub fn left_context(&self) -> &'a [TranscriptSegment] {
        self.left_context
    }

    pub fn owned_region(&self) -> &'a [TranscriptSegment] {
        self.owned_region
    }

    pub fn right_context(&self) -> &'a [TranscriptSegment] {
        self.right_context
    }
}

/// One annotation view over complete restored transcript spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoredTranscriptWindow<'a> {
    left_context: &'a [RestoredTranscriptSpan],
    owned_region: &'a [RestoredTranscriptSpan],
    right_context: &'a [RestoredTranscriptSpan],
}

impl<'a> RestoredTranscriptWindow<'a> {
    pub fn left_context(&self) -> &'a [RestoredTranscriptSpan] {
        self.left_context
    }

    pub fn owned_region(&self) -> &'a [RestoredTranscriptSpan] {
        self.owned_region
    }

    pub fn right_context(&self) -> &'a [RestoredTranscriptSpan] {
        self.right_context
    }

    /// Concatenates readable owned text while retaining omission provenance in
    /// `owned_region` for callers that need it.
    pub fn owned_text(&self) -> String {
        self.owned_region
            .iter()
            .filter_map(restored_span_text)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedRegionPartitionError {
    EmptyOwnedRegion,
    EndBeforeStart {
        start: TranscriptSegmentId,
        end: TranscriptSegmentId,
    },
    OutsideOwnedRegion {
        owned_start: TranscriptSegmentId,
        owned_end: TranscriptSegmentId,
        start: TranscriptSegmentId,
        end: TranscriptSegmentId,
    },
    CoverageMismatch {
        expected: TranscriptSegmentId,
        actual: TranscriptSegmentId,
    },
    UnexpectedRange {
        actual: TranscriptSegmentId,
    },
    UncoveredTail {
        expected: TranscriptSegmentId,
    },
}

pub(crate) fn validate_owned_region_partition(
    owned_region: &[TranscriptSegment],
    ranges: impl IntoIterator<Item = (TranscriptSegmentId, TranscriptSegmentId)>,
) -> Result<(), OwnedRegionPartitionError> {
    let Some((owned_start, owned_end)) = owned_region
        .first()
        .zip(owned_region.last())
        .map(|(first, last)| (first.id, last.id))
    else {
        return Err(OwnedRegionPartitionError::EmptyOwnedRegion);
    };
    let owned_start_position = owned_start.index();
    let owned_end_position = owned_end.index();
    let mut next_uncovered = 0;

    for (start, end) in ranges {
        let start_position = start.index();
        let end_position = end.index();
        if end_position < start_position {
            return Err(OwnedRegionPartitionError::EndBeforeStart { start, end });
        }
        if start_position < owned_start_position || end_position > owned_end_position {
            return Err(OwnedRegionPartitionError::OutsideOwnedRegion {
                owned_start,
                owned_end,
                start,
                end,
            });
        }
        let Some(expected) = owned_region.get(next_uncovered) else {
            return Err(OwnedRegionPartitionError::UnexpectedRange { actual: start });
        };
        if start != expected.id {
            return Err(OwnedRegionPartitionError::CoverageMismatch {
                expected: expected.id,
                actual: start,
            });
        }

        next_uncovered = end_position - owned_start_position + 1;
    }

    if let Some(expected) = owned_region.get(next_uncovered) {
        return Err(OwnedRegionPartitionError::UncoveredTail {
            expected: expected.id,
        });
    }

    Ok(())
}

pub fn build_windows(
    sources: &ValidatedSources,
    config: WindowingConfig,
) -> Vec<TranscriptWindow<'_>> {
    let segments = &sources.transcript().segments;
    let mut windows = Vec::new();
    let mut owned_start = 0;

    while owned_start < segments.len() {
        let mut owned_end = owned_start;
        let mut character_count = 0_usize;
        while let Some(segment) = segments.get(owned_end) {
            let next_character_count = character_count.saturating_add(segment.text.chars().count());
            let next_duration = Duration::from_millis(
                segment
                    .end_ms
                    .saturating_sub(segments[owned_start].start_ms),
            );
            let exceeds_budget = next_character_count > config.max_owned_characters.get()
                || next_duration > config.max_owned_duration;
            if owned_end > owned_start && exceeds_budget {
                break;
            }
            character_count = next_character_count;
            owned_end += 1;
        }

        let mut visible_start = owned_start;
        let mut left_context_characters = 0_usize;
        while visible_start > 0 {
            let next_character_count = left_context_characters
                .saturating_add(segments[visible_start - 1].text.chars().count());
            if next_character_count > config.context_characters {
                break;
            }
            left_context_characters = next_character_count;
            visible_start -= 1;
        }

        let mut visible_end = owned_end;
        let mut right_context_characters = 0_usize;
        while let Some(segment) = segments.get(visible_end) {
            let next_character_count =
                right_context_characters.saturating_add(segment.text.chars().count());
            if next_character_count > config.context_characters {
                break;
            }
            right_context_characters = next_character_count;
            visible_end += 1;
        }

        windows.push(TranscriptWindow {
            left_context: &segments[visible_start..owned_start],
            owned_region: &segments[owned_start..owned_end],
            right_context: &segments[owned_end..visible_end],
        });
        owned_start = owned_end;
    }

    windows
}

/// Builds annotation windows without splitting restored transcript spans.
///
/// Character budgets count only readable restored text. Omitted disfluencies
/// remain in an adjacent owned region so the window sequence still preserves
/// complete transcript-segment provenance.
pub fn build_restored_windows<'a>(
    sources: &ValidatedSources,
    restored_transcript: &'a RestoredTranscript,
    config: WindowingConfig,
) -> Result<Vec<RestoredTranscriptWindow<'a>>, RestoredWindowingError> {
    validate_restored_transcript(sources, restored_transcript)?;

    let spans = &restored_transcript.spans;
    let segments = &sources.transcript().segments;
    let mut windows = Vec::new();
    let mut owned_start = 0;

    while owned_start < spans.len() {
        let first_source = spans[owned_start].source_start().index();
        let mut owned_end = owned_start;
        let mut character_count = 0_usize;
        let mut contains_text = false;
        while let Some(span) = spans.get(owned_end) {
            let span_text = restored_span_text(span);
            let next_character_count =
                character_count.saturating_add(span_text.map_or(0, |text| text.chars().count()));
            let next_duration = Duration::from_millis(
                segments[span.source_end().index()]
                    .end_ms
                    .saturating_sub(segments[first_source].start_ms),
            );
            let exceeds_budget = next_character_count > config.max_owned_characters.get()
                || next_duration > config.max_owned_duration;
            if span_text.is_some() && contains_text && exceeds_budget {
                break;
            }
            character_count = next_character_count;
            contains_text |= span_text.is_some();
            owned_end += 1;
        }

        let mut visible_start = owned_start;
        let mut left_context_characters = 0_usize;
        while visible_start > 0 {
            let next_character_count = left_context_characters.saturating_add(
                restored_span_text(&spans[visible_start - 1])
                    .map_or(0, |text| text.chars().count()),
            );
            if next_character_count > config.context_characters {
                break;
            }
            left_context_characters = next_character_count;
            visible_start -= 1;
        }

        let mut visible_end = owned_end;
        let mut right_context_characters = 0_usize;
        while let Some(span) = spans.get(visible_end) {
            let next_character_count = right_context_characters
                .saturating_add(restored_span_text(span).map_or(0, |text| text.chars().count()));
            if next_character_count > config.context_characters {
                break;
            }
            right_context_characters = next_character_count;
            visible_end += 1;
        }

        windows.push(RestoredTranscriptWindow {
            left_context: &spans[visible_start..owned_start],
            owned_region: &spans[owned_start..owned_end],
            right_context: &spans[owned_end..visible_end],
        });
        owned_start = owned_end;
    }

    Ok(windows)
}

fn restored_span_text(span: &RestoredTranscriptSpan) -> Option<&str> {
    match span {
        RestoredTranscriptSpan::Text { text, .. } => Some(text),
        RestoredTranscriptSpan::OmittedDisfluency { .. } => None,
    }
}

pub(crate) fn validate_restored_transcript(
    sources: &ValidatedSources,
    restored_transcript: &RestoredTranscript,
) -> Result<(), RestoredWindowingError> {
    let segments = &sources.transcript().segments;
    let mut next_source = 0;

    for (span_index, span) in restored_transcript.spans.iter().enumerate() {
        let source_start = span.source_start();
        let source_end = span.source_end();
        if source_end.index() < source_start.index() {
            return Err(RestoredWindowingError::SpanEndBeforeStart {
                span_index,
                source_start,
                source_end,
            });
        }
        if source_start.index() >= segments.len() || source_end.index() >= segments.len() {
            return Err(RestoredWindowingError::SpanOutsideTranscript {
                span_index,
                source_start,
                source_end,
                segment_count: segments.len(),
            });
        }
        let Some(expected) = segments.get(next_source) else {
            return Err(RestoredWindowingError::UnexpectedSpan {
                span_index,
                source_start,
            });
        };
        if source_start != expected.id {
            return Err(RestoredWindowingError::CoverageMismatch {
                span_index,
                expected: expected.id,
                actual: source_start,
            });
        }
        if let Some(text) = restored_span_text(span)
            && text.trim().is_empty()
        {
            return Err(RestoredWindowingError::EmptyText {
                span_index,
                source_start,
            });
        }
        next_source = source_end.index() + 1;
    }

    if let Some(expected) = segments.get(next_source) {
        return Err(RestoredWindowingError::UncoveredTranscriptTail {
            expected: expected.id,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoredWindowingError {
    SpanEndBeforeStart {
        span_index: usize,
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
    },
    SpanOutsideTranscript {
        span_index: usize,
        source_start: TranscriptSegmentId,
        source_end: TranscriptSegmentId,
        segment_count: usize,
    },
    CoverageMismatch {
        span_index: usize,
        expected: TranscriptSegmentId,
        actual: TranscriptSegmentId,
    },
    UnexpectedSpan {
        span_index: usize,
        source_start: TranscriptSegmentId,
    },
    UncoveredTranscriptTail {
        expected: TranscriptSegmentId,
    },
    EmptyText {
        span_index: usize,
        source_start: TranscriptSegmentId,
    },
}

impl fmt::Display for RestoredWindowingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpanEndBeforeStart {
                span_index,
                source_start,
                source_end,
            } => write!(
                formatter,
                "restored transcript span {span_index} ends at source segment {} before it starts at {}",
                source_end.0, source_start.0
            ),
            Self::SpanOutsideTranscript {
                span_index,
                source_start,
                source_end,
                segment_count,
            } => write!(
                formatter,
                "restored transcript span {span_index} covers source segments {} through {}, outside a transcript containing {segment_count} segments",
                source_start.0, source_end.0
            ),
            Self::CoverageMismatch {
                span_index,
                expected,
                actual,
            } => write!(
                formatter,
                "restored transcript span {span_index} should begin at source segment {} but begins at {}",
                expected.0, actual.0
            ),
            Self::UnexpectedSpan {
                span_index,
                source_start,
            } => write!(
                formatter,
                "restored transcript span {span_index} unexpectedly begins at source segment {} after the transcript is already covered",
                source_start.0
            ),
            Self::UncoveredTranscriptTail { expected } => write!(
                formatter,
                "restored transcript leaves source segment {} and the remaining transcript tail uncovered",
                expected.0
            ),
            Self::EmptyText {
                span_index,
                source_start,
            } => write!(
                formatter,
                "restored transcript span {span_index} starting at source segment {} has empty text",
                source_start.0
            ),
        }
    }
}

impl Error for RestoredWindowingError {}
