use std::{error::Error, fmt, ops::Range};

use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices};

const MAX_CHANGED_PERCENT: usize = 5;

/// Source-backed passage ranges recovered from a model-proposed text partition.
#[derive(Debug, Clone, PartialEq)]
pub struct PassageProjection {
    byte_ranges: Vec<Range<usize>>,
    changed_characters: usize,
    compared_characters: usize,
}

impl PassageProjection {
    /// Contiguous UTF-8 byte ranges that partition the authoritative source.
    pub fn byte_ranges(&self) -> &[Range<usize>] {
        &self.byte_ranges
    }

    pub fn into_byte_ranges(self) -> Vec<Range<usize>> {
        self.byte_ranges
    }

    pub const fn changed_characters(&self) -> usize {
        self.changed_characters
    }

    /// The larger Unicode-character length of the source and proposed text.
    pub const fn compared_characters(&self) -> usize {
        self.compared_characters
    }

    pub fn error_ratio(&self) -> f64 {
        self.changed_characters as f64 / self.compared_characters as f64
    }

    pub const fn is_exact(&self) -> bool {
        self.changed_characters == 0
    }

    pub const fn diagnostics(&self) -> PassageProjectionDiagnostics {
        PassageProjectionDiagnostics {
            changed_characters: self.changed_characters,
            compared_characters: self.compared_characters,
        }
    }
}

/// Persistable quality measurements for one accepted passage projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct PassageProjectionDiagnostics {
    pub changed_characters: usize,
    pub compared_characters: usize,
}

impl PassageProjectionDiagnostics {
    pub const fn is_exact(self) -> bool {
        self.changed_characters == 0
    }

    pub fn error_ratio(self) -> f64 {
        self.changed_characters as f64 / self.compared_characters as f64
    }
}

/// Projects model-proposed passage text onto authoritative restored text.
///
/// The proposed text is used only to recover passage boundaries. Returned
/// ranges always slice `source`, so accepted copying mistakes cannot alter the
/// displayed transcript. Differences of 5% or more are rejected.
pub fn project_passage_boundaries<'a>(
    source: &str,
    proposed_passages: impl IntoIterator<Item = &'a str>,
) -> Result<PassageProjection, PassageProjectionError> {
    if source.is_empty() {
        return Err(PassageProjectionError::EmptySource);
    }

    let proposed_passages: Vec<_> = proposed_passages.into_iter().collect();
    if proposed_passages.is_empty() {
        return Err(PassageProjectionError::NoPassages);
    }

    let mut proposed_boundaries = Vec::with_capacity(proposed_passages.len() + 1);
    proposed_boundaries.push(0);
    let mut proposed_character_count = 0;
    for (index, passage) in proposed_passages.iter().enumerate() {
        let character_count = passage.chars().count();
        if character_count == 0 {
            return Err(PassageProjectionError::EmptyProposedPassage { index });
        }
        proposed_character_count += character_count;
        proposed_boundaries.push(proposed_character_count);
    }

    let source_characters: Vec<_> = source.chars().collect();
    let proposed_characters: Vec<_> = proposed_passages
        .iter()
        .flat_map(|passage| passage.chars())
        .collect();
    let operations =
        capture_diff_slices(Algorithm::Myers, &source_characters, &proposed_characters);
    let changed_characters: usize = operations
        .iter()
        .filter(|operation| operation.tag() != DiffTag::Equal)
        .map(|operation| operation.old_range().len().max(operation.new_range().len()))
        .sum();
    let compared_characters = source_characters.len().max(proposed_characters.len());

    if changed_characters.saturating_mul(100)
        >= compared_characters.saturating_mul(MAX_CHANGED_PERCENT)
    {
        return Err(PassageProjectionError::DifferenceTooLarge {
            changed_characters,
            compared_characters,
        });
    }

    let source_positions = project_character_positions(
        &operations,
        source_characters.len(),
        proposed_characters.len(),
    );
    let source_byte_offsets: Vec<_> = source
        .char_indices()
        .map(|(byte_offset, _)| byte_offset)
        .chain(std::iter::once(source.len()))
        .collect();
    let mut byte_ranges = Vec::with_capacity(proposed_passages.len());
    for (index, proposed_range) in proposed_boundaries.windows(2).enumerate() {
        let source_start = source_positions[proposed_range[0]];
        let source_end = source_positions[proposed_range[1]];
        if source_start == source_end {
            return Err(PassageProjectionError::EmptyProjectedPassage { index });
        }
        if source_start > source_end {
            return Err(PassageProjectionError::NonMonotonicProjection { index });
        }
        byte_ranges.push(source_byte_offsets[source_start]..source_byte_offsets[source_end]);
    }

    Ok(PassageProjection {
        byte_ranges,
        changed_characters,
        compared_characters,
    })
}

fn project_character_positions(
    operations: &[similar::DiffOp],
    source_len: usize,
    proposed_len: usize,
) -> Vec<usize> {
    let mut source_positions = vec![0; proposed_len + 1];
    let mut next_unprojected = 0;

    for operation in operations {
        let source_range = operation.old_range();
        let proposed_range = operation.new_range();
        let source_range_len = source_range.len();
        let proposed_range_len = proposed_range.len();

        for offset in 0..=proposed_range_len {
            let proposed_position = proposed_range.start + offset;
            if proposed_position < next_unprojected {
                continue;
            }
            let source_offset = offset
                .saturating_mul(source_range_len)
                .saturating_add(proposed_range_len / 2)
                / proposed_range_len.max(1);
            source_positions[proposed_position] = source_range.start + source_offset;
            next_unprojected = proposed_position + 1;
        }
    }

    // Text missing at either outer edge still belongs to the first or last
    // passage; the partition must cover the complete authoritative source.
    source_positions[0] = 0;
    source_positions[proposed_len] = source_len;
    source_positions
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassageProjectionError {
    EmptySource,
    NoPassages,
    EmptyProposedPassage {
        index: usize,
    },
    DifferenceTooLarge {
        changed_characters: usize,
        compared_characters: usize,
    },
    EmptyProjectedPassage {
        index: usize,
    },
    NonMonotonicProjection {
        index: usize,
    },
}

impl fmt::Display for PassageProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySource => formatter.write_str("cannot partition empty restored text"),
            Self::NoPassages => formatter.write_str("the model proposed no lecture passages"),
            Self::EmptyProposedPassage { index } => {
                write!(formatter, "proposed lecture passage {index} is empty")
            }
            Self::DifferenceTooLarge {
                changed_characters,
                compared_characters,
            } => write!(
                formatter,
                "the proposed passage text changed {changed_characters} of {compared_characters} characters, exceeding the repair limit"
            ),
            Self::EmptyProjectedPassage { index } => write!(
                formatter,
                "proposed lecture passage {index} projects to an empty source range"
            ),
            Self::NonMonotonicProjection { index } => write!(
                formatter,
                "proposed lecture passage {index} projects before its starting source position"
            ),
        }
    }
}

impl Error for PassageProjectionError {}
