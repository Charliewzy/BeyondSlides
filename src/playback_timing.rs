use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices};
use unicode_normalization::UnicodeNormalization;

use crate::{RestoredLecturePassage, TimedTranscript, TimedTranscriptToken, Transcript};

const MIN_MATCHED_PERCENT: usize = 60;

/// Why a passage received its playback interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackTimingBasis {
    /// Both boundaries were projected from fine-grained ASR timing.
    TimedTokens,
    /// Fine-grained alignment was unavailable or too weak, so the passage uses
    /// its complete transcript-segment evidence envelope.
    TranscriptSegments,
}

/// An approximate interval used to play one lecture passage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct PassagePlaybackInterval {
    pub start_ms: u64,
    pub end_ms: u64,
    pub basis: PlaybackTimingBasis,
}

/// Projects readable passage text onto fine-grained ASR timing.
///
/// Passages with overlapping transcript provenance are aligned together and in
/// order within the union of their coarse evidence envelopes. Leading and
/// trailing source-only tokens therefore help locate a passage that splits a
/// restored transcript span without counting against the match quality.
/// Adjacent token-derived intervals share one boundary.
pub fn project_passage_playback_intervals(
    transcript: &Transcript,
    passages: &[RestoredLecturePassage],
    timing: &TimedTranscript,
) -> Result<Vec<PassagePlaybackInterval>, PlaybackTimingError> {
    validate_timing(timing)?;

    let coarse_intervals = passages
        .iter()
        .enumerate()
        .map(|(passage_index, passage)| coarse_interval(transcript, passage_index, passage))
        .collect::<Result<Vec<_>, PlaybackTimingError>>()?;
    let mut intervals = coarse_intervals.clone();

    let mut group_start = 0;
    while group_start < passages.len() {
        let mut group_end = group_start + 1;
        let mut furthest_source_end = passages[group_start].source_end.index();
        while group_end < passages.len()
            && passages[group_end].source_start.index() <= furthest_source_end
        {
            furthest_source_end = furthest_source_end.max(passages[group_end].source_end.index());
            group_end += 1;
        }

        let group_passages = &passages[group_start..group_end];
        let group_coarse = &coarse_intervals[group_start..group_end];
        if let Some(projected) = project_passage_group(group_passages, group_coarse, &timing.tokens)
        {
            intervals[group_start..group_end].copy_from_slice(&projected);
        }
        group_start = group_end;
    }

    for boundary_index in 0..intervals.len().saturating_sub(1) {
        let (left, right) = intervals.split_at_mut(boundary_index + 1);
        let left = &mut left[boundary_index];
        let right = &mut right[0];
        if left.basis != PlaybackTimingBasis::TimedTokens
            || right.basis != PlaybackTimingBasis::TimedTokens
        {
            continue;
        }

        let boundary = midpoint(left.end_ms, right.start_ms);
        if boundary > left.start_ms && boundary < right.end_ms {
            left.end_ms = boundary;
            right.start_ms = boundary;
        }
    }

    Ok(intervals)
}

fn validate_timing(timing: &TimedTranscript) -> Result<(), PlaybackTimingError> {
    if timing.tokens.is_empty() {
        return Err(PlaybackTimingError::NoTimedTokens);
    }

    let mut previous_end = None;
    for (token_index, token) in timing.tokens.iter().enumerate() {
        if token.text.trim().is_empty() {
            return Err(PlaybackTimingError::EmptyTimedToken { token_index });
        }
        if token.end_ms <= token.start_ms {
            return Err(PlaybackTimingError::InvalidTimedTokenInterval {
                token_index,
                start_ms: token.start_ms,
                end_ms: token.end_ms,
            });
        }
        if let Some(previous_end_ms) = previous_end
            && token.start_ms < previous_end_ms
        {
            return Err(PlaybackTimingError::OverlappingTimedTokens {
                token_index,
                previous_end_ms,
                start_ms: token.start_ms,
            });
        }
        previous_end = Some(token.end_ms);
    }
    Ok(())
}

fn coarse_interval(
    transcript: &Transcript,
    passage_index: usize,
    passage: &RestoredLecturePassage,
) -> Result<PassagePlaybackInterval, PlaybackTimingError> {
    let first = transcript
        .segments
        .get(passage.source_start.index())
        .filter(|segment| segment.id == passage.source_start)
        .ok_or(PlaybackTimingError::UnknownPassageSource {
            passage_index,
            source: passage.source_start,
        })?;
    let last = transcript
        .segments
        .get(passage.source_end.index())
        .filter(|segment| segment.id == passage.source_end)
        .ok_or(PlaybackTimingError::UnknownPassageSource {
            passage_index,
            source: passage.source_end,
        })?;
    if passage.source_end.index() < passage.source_start.index() {
        return Err(PlaybackTimingError::ReversedPassageSource {
            passage_index,
            start: passage.source_start,
            end: passage.source_end,
        });
    }
    let (start_ms, end_ms) = first
        .start_ms
        .zip(last.end_ms)
        .ok_or(PlaybackTimingError::MissingTranscriptTiming { passage_index })?;
    Ok(PassagePlaybackInterval {
        start_ms,
        end_ms,
        basis: PlaybackTimingBasis::TranscriptSegments,
    })
}

fn project_passage_group(
    passages: &[RestoredLecturePassage],
    coarse_intervals: &[PassagePlaybackInterval],
    tokens: &[TimedTranscriptToken],
) -> Option<Vec<PassagePlaybackInterval>> {
    let coarse_start = coarse_intervals
        .iter()
        .map(|interval| interval.start_ms)
        .min()?;
    let coarse_end = coarse_intervals
        .iter()
        .map(|interval| interval.end_ms)
        .max()?;
    let relevant_tokens: Vec<_> = tokens
        .iter()
        .filter(|token| token.end_ms > coarse_start && token.start_ms < coarse_end)
        .collect();
    let timed_text = TimedText::new(&relevant_tokens)?;
    let mut passage_boundaries = Vec::with_capacity(passages.len() + 1);
    let mut passage_characters = Vec::new();
    passage_boundaries.push(0);
    for passage in passages {
        passage_characters.extend(normalized_characters(&passage.text));
        passage_boundaries.push(passage_characters.len());
    }

    let operations = capture_diff_slices(
        Algorithm::Myers,
        &timed_text.characters,
        &passage_characters,
    );
    for passage_range in passage_boundaries.windows(2) {
        let passage_start = passage_range[0];
        let passage_end = passage_range[1];
        if passage_start == passage_end {
            return None;
        }
        let matched_characters: usize = operations
            .iter()
            .filter(|operation| operation.tag() == DiffTag::Equal)
            .map(|operation| {
                let equal = operation.new_range();
                equal
                    .end
                    .min(passage_end)
                    .saturating_sub(equal.start.max(passage_start))
            })
            .sum();
        if matched_characters.saturating_mul(100)
            < (passage_end - passage_start).saturating_mul(MIN_MATCHED_PERCENT)
        {
            return None;
        }
    }

    let source_positions = project_candidate_boundaries(&operations, passage_characters.len())?;
    passage_boundaries
        .windows(2)
        .zip(coarse_intervals)
        .map(|(passage_range, coarse)| {
            let start_ms =
                timed_text.boundary_times[source_positions[passage_range[0]]].max(coarse.start_ms);
            let end_ms =
                timed_text.boundary_times[source_positions[passage_range[1]]].min(coarse.end_ms);
            (end_ms > start_ms).then_some(PassagePlaybackInterval {
                start_ms,
                end_ms,
                basis: PlaybackTimingBasis::TimedTokens,
            })
        })
        .collect()
}

fn project_candidate_boundaries(
    operations: &[similar::DiffOp],
    candidate_len: usize,
) -> Option<Vec<usize>> {
    let mut source_positions = vec![None; candidate_len + 1];
    for operation in operations {
        let source_range = operation.old_range();
        let candidate_range = operation.new_range();
        if candidate_range.is_empty() {
            continue;
        }

        for offset in 0..=candidate_range.len() {
            let source_offset = offset
                .saturating_mul(source_range.len())
                .saturating_add(candidate_range.len() / 2)
                / candidate_range.len();
            source_positions[candidate_range.start + offset] =
                Some(source_range.start + source_offset);
        }
    }
    source_positions.into_iter().collect()
}

struct TimedText {
    characters: Vec<char>,
    boundary_times: Vec<u64>,
}

impl TimedText {
    fn new(tokens: &[&TimedTranscriptToken]) -> Option<Self> {
        let mut characters = Vec::new();
        let mut boundary_times = Vec::new();
        let mut previous_end = None;

        for token in tokens {
            let token_characters = normalized_characters(&token.text);
            if token_characters.is_empty() {
                continue;
            }
            if let Some(previous_end_ms) = previous_end {
                *boundary_times.last_mut()? = midpoint(previous_end_ms, token.start_ms);
            } else {
                boundary_times.push(token.start_ms);
            }

            let character_count = token_characters.len() as u64;
            let duration = token.end_ms - token.start_ms;
            characters.extend(token_characters);
            boundary_times.extend(
                (1..=character_count).map(|offset| {
                    token.start_ms + duration.saturating_mul(offset) / character_count
                }),
            );
            previous_end = Some(token.end_ms);
        }

        (!characters.is_empty()).then_some(Self {
            characters,
            boundary_times,
        })
    }
}

fn normalized_characters(text: &str) -> Vec<char> {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

const fn midpoint(left: u64, right: u64) -> u64 {
    left / 2 + right / 2 + (left % 2 + right % 2).div_ceil(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackTimingError {
    MissingTranscriptTiming {
        passage_index: usize,
    },
    NoTimedTokens,
    EmptyTimedToken {
        token_index: usize,
    },
    InvalidTimedTokenInterval {
        token_index: usize,
        start_ms: u64,
        end_ms: u64,
    },
    OverlappingTimedTokens {
        token_index: usize,
        previous_end_ms: u64,
        start_ms: u64,
    },
    UnknownPassageSource {
        passage_index: usize,
        source: crate::TranscriptSegmentId,
    },
    ReversedPassageSource {
        passage_index: usize,
        start: crate::TranscriptSegmentId,
        end: crate::TranscriptSegmentId,
    },
}

impl fmt::Display for PlaybackTimingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTranscriptTiming { passage_index } => write!(
                formatter,
                "passage {passage_index} has no source timestamps for audio projection"
            ),
            Self::NoTimedTokens => formatter.write_str("timed transcript contains no tokens"),
            Self::EmptyTimedToken { token_index } => {
                write!(
                    formatter,
                    "timed transcript token {token_index} has empty text"
                )
            }
            Self::InvalidTimedTokenInterval {
                token_index,
                start_ms,
                end_ms,
            } => write!(
                formatter,
                "timed transcript token {token_index} starts at {start_ms} ms and ends at {end_ms} ms"
            ),
            Self::OverlappingTimedTokens {
                token_index,
                previous_end_ms,
                start_ms,
            } => write!(
                formatter,
                "timed transcript token {token_index} starts at {start_ms} ms before the previous token ends at {previous_end_ms} ms"
            ),
            Self::UnknownPassageSource {
                passage_index,
                source,
            } => write!(
                formatter,
                "lecture passage {passage_index} references unknown transcript segment {}",
                source.0
            ),
            Self::ReversedPassageSource {
                passage_index,
                start,
                end,
            } => write!(
                formatter,
                "lecture passage {passage_index} source range {}..={} is reversed",
                start.0, end.0
            ),
        }
    }
}

impl Error for PlaybackTimingError {}
