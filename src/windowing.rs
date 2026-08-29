use std::{error::Error, fmt, num::NonZeroUsize, time::Duration};

use crate::{TranscriptSentence, ValidatedSources};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowingConfig {
    max_owned_characters: NonZeroUsize,
    max_owned_duration: Duration,
    context_characters: usize,
}

impl WindowingConfig {
    /// Configures complete-sentence windows using Unicode character and
    /// wall-clock budgets. An individually oversized sentence remains intact.
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
    left_context: &'a [TranscriptSentence],
    owned_region: &'a [TranscriptSentence],
    right_context: &'a [TranscriptSentence],
}

impl<'a> TranscriptWindow<'a> {
    pub fn left_context(&self) -> &'a [TranscriptSentence] {
        self.left_context
    }

    pub fn owned_region(&self) -> &'a [TranscriptSentence] {
        self.owned_region
    }

    pub fn right_context(&self) -> &'a [TranscriptSentence] {
        self.right_context
    }
}

pub fn build_windows(
    sources: &ValidatedSources,
    config: WindowingConfig,
) -> Vec<TranscriptWindow<'_>> {
    let sentences = &sources.transcript().sentences;
    let mut windows = Vec::new();
    let mut owned_start = 0;

    while owned_start < sentences.len() {
        let mut owned_end = owned_start;
        let mut character_count = 0_usize;
        while let Some(sentence) = sentences.get(owned_end) {
            let next_character_count =
                character_count.saturating_add(sentence.text.chars().count());
            let next_duration = Duration::from_millis(
                sentence
                    .end_ms
                    .saturating_sub(sentences[owned_start].start_ms),
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
                .saturating_add(sentences[visible_start - 1].text.chars().count());
            if next_character_count > config.context_characters {
                break;
            }
            left_context_characters = next_character_count;
            visible_start -= 1;
        }

        let mut visible_end = owned_end;
        let mut right_context_characters = 0_usize;
        while let Some(sentence) = sentences.get(visible_end) {
            let next_character_count =
                right_context_characters.saturating_add(sentence.text.chars().count());
            if next_character_count > config.context_characters {
                break;
            }
            right_context_characters = next_character_count;
            visible_end += 1;
        }

        windows.push(TranscriptWindow {
            left_context: &sentences[visible_start..owned_start],
            owned_region: &sentences[owned_start..owned_end],
            right_context: &sentences[owned_end..visible_end],
        });
        owned_start = owned_end;
    }

    windows
}
