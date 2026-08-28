use std::{error::Error, fmt, num::NonZeroUsize};

use crate::{TranscriptSentence, ValidatedSources};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowingConfig {
    max_owned_sentences: NonZeroUsize,
    context_sentences: usize,
}

impl WindowingConfig {
    pub fn new(
        max_owned_sentences: usize,
        context_sentences: usize,
    ) -> Result<Self, WindowingConfigError> {
        let Some(max_owned_sentences) = NonZeroUsize::new(max_owned_sentences) else {
            return Err(WindowingConfigError::EmptyOwnedRegion);
        };
        Ok(Self {
            max_owned_sentences,
            context_sentences,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowingConfigError {
    EmptyOwnedRegion,
}

impl fmt::Display for WindowingConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOwnedRegion => {
                write!(
                    formatter,
                    "an owned region must contain at least one sentence"
                )
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
    let max_owned_sentences = config.max_owned_sentences.get();

    (0..sentences.len())
        .step_by(max_owned_sentences)
        .map(|owned_start| {
            let owned_end = owned_start
                .saturating_add(max_owned_sentences)
                .min(sentences.len());
            let visible_start = owned_start.saturating_sub(config.context_sentences);
            let visible_end = owned_end
                .saturating_add(config.context_sentences)
                .min(sentences.len());

            TranscriptWindow {
                left_context: &sentences[visible_start..owned_start],
                owned_region: &sentences[owned_start..owned_end],
                right_context: &sentences[owned_end..visible_end],
            }
        })
        .collect()
}
