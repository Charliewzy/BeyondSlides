use std::{error::Error, fmt, num::NonZeroUsize, time::Duration};

use crate::{TranscriptSegment, ValidatedSources};

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
