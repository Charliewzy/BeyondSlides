//! Source-backed semantic segmentation. Requests own gaps, not passage ranges.

use std::{error::Error, fmt, ops::Range};

use serde::{Deserialize, Serialize};

use crate::{
    AnnotationDiagnostics, RestoredLecturePassage, RestoredTranscript, Score5, SlideScorer,
    ValidatedRestoredAnalysis, ValidatedSources, infer_slide_positions,
    restored_annotation::source_provenance, windowing::validate_restored_transcript,
};

const OWNED_BOUNDARIES: usize = 48;
const CONTEXT_ATOMS: usize = 8;
const WINDOWS_PER_BATCH: usize = 2;
const LONG_ATOM_CHARACTERS: usize = 180;
const MAX_ATOM_CHARACTERS: usize = 150;
pub const BOUNDARY_MAX_PASSAGE_CHARACTERS: usize = 300;

pub const PASSAGE_BOUNDARY_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/passage_boundaries.md"
));

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryDecision {
    pub after_atom: usize,
    pub cut_cost: Score5,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryWindowDecision {
    pub window_index: usize,
    pub boundaries: Vec<BoundaryDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedBoundaryBatch {
    pub windows: Vec<BoundaryWindowDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryBatchResult {
    pub batch_index: usize,
    pub decisions: ProposedBoundaryBatch,
    pub diagnostics: AnnotationDiagnostics,
}

/// Constructed by a plan so every owned gap has both neighboring atoms visible.
#[derive(Debug, Clone, Serialize)]
pub struct BoundaryBatchTask {
    #[serde(skip)]
    batch_index: usize,
    windows: Vec<BoundaryWindow>,
}

#[derive(Debug, Clone, Serialize)]
struct BoundaryWindow {
    window_index: usize,
    owned_boundary_after_atom_ids: Vec<usize>,
    atoms: Vec<VisibleAtom>,
}

#[derive(Debug, Clone, Serialize)]
struct VisibleAtom {
    atom_id: usize,
    text: String,
}

impl BoundaryBatchTask {
    pub fn batch_index(&self) -> usize {
        self.batch_index
    }

    /// Validates coverage, identity, and uniqueness even for persisted results.
    pub fn validate(&self, proposed: &ProposedBoundaryBatch) -> Result<(), BoundaryError> {
        if proposed.windows.len() != self.windows.len() {
            return Err(BoundaryError::InvalidResponse("wrong window count".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for window in &proposed.windows {
            let expected = self
                .windows
                .iter()
                .find(|w| w.window_index == window.window_index)
                .ok_or_else(|| {
                    BoundaryError::InvalidResponse(format!(
                        "unknown window {}",
                        window.window_index
                    ))
                })?;
            if !seen.insert(window.window_index) {
                return Err(BoundaryError::InvalidResponse(format!(
                    "duplicate window {}",
                    window.window_index
                )));
            }
            let mut actual: Vec<_> = window.boundaries.iter().map(|b| b.after_atom).collect();
            actual.sort_unstable();
            if actual != expected.owned_boundary_after_atom_ids {
                return Err(BoundaryError::InvalidResponse(format!(
                    "window {} must classify each owned boundary exactly once: {:?}; received {:?}",
                    window.window_index, expected.owned_boundary_after_atom_ids, actual
                )));
            }
        }
        Ok(())
    }
}

/// Punctuation supplies ordinary cuts, while long atoms receive UTF-8-safe
/// fallback cuts. Model costs and global DP select the least damaging exact
/// source partition.
pub struct BoundarySegmentationPlan {
    text: String,
    atoms: Vec<Range<usize>>,
    tasks: Vec<BoundaryBatchTask>,
}

impl BoundarySegmentationPlan {
    pub fn new(restored: &RestoredTranscript) -> Self {
        let text = restored.text();
        let atoms = atomize(&text);
        let gap_count = atoms.len().saturating_sub(1);
        let mut windows = Vec::new();
        for start in (0..gap_count).step_by(OWNED_BOUNDARIES) {
            let end = (start + OWNED_BOUNDARIES).min(gap_count);
            let visible_start = start.saturating_sub(CONTEXT_ATOMS);
            let visible_end = (end + 1 + CONTEXT_ATOMS).min(atoms.len());
            windows.push(BoundaryWindow {
                window_index: windows.len(),
                owned_boundary_after_atom_ids: (start..end).collect(),
                atoms: (visible_start..visible_end)
                    .map(|atom_id| VisibleAtom {
                        atom_id,
                        text: text[atoms[atom_id].clone()].to_owned(),
                    })
                    .collect(),
            });
        }
        let tasks = windows
            .chunks(WINDOWS_PER_BATCH)
            .enumerate()
            .map(|(batch_index, windows)| BoundaryBatchTask {
                batch_index,
                windows: windows.to_vec(),
            })
            .collect();
        Self { text, atoms, tasks }
    }

    pub fn tasks(&self) -> &[BoundaryBatchTask] {
        &self.tasks
    }

    /// No model-copied text is accepted: returned byte ranges partition the
    /// authoritative restored text, independent of request-window edges.
    pub fn partition(
        &self,
        results: &[BoundaryBatchResult],
    ) -> Result<Vec<Range<usize>>, BoundaryError> {
        if results.len() != self.tasks.len() {
            return Err(BoundaryError::InvalidResponse(
                "incomplete boundary batches".into(),
            ));
        }
        let mut seen = vec![false; self.tasks.len()];
        let mut cut_costs = vec![Score5::ZERO; self.atoms.len().saturating_sub(1)];
        for result in results {
            let task = self
                .tasks
                .get(result.batch_index)
                .ok_or_else(|| BoundaryError::InvalidResponse("unknown batch index".into()))?;
            if seen[result.batch_index] {
                return Err(BoundaryError::InvalidResponse(
                    "duplicate batch index".into(),
                ));
            }
            seen[result.batch_index] = true;
            task.validate(&result.decisions)?;
            for window in &result.decisions.windows {
                for boundary in &window.boundaries {
                    cut_costs[boundary.after_atom] = boundary.cut_cost;
                }
            }
        }
        partition(&self.text, &self.atoms, &cut_costs)
    }

    /// Attaches coarse source provenance and newly inferred slide positions to
    /// the new passages. Novelty retrieves its own slide evidence later; this
    /// stage does not pretend retrieval hits are model-judged related slides.
    pub fn assemble(
        &self,
        sources: ValidatedSources,
        restored: RestoredTranscript,
        scorer: &dyn SlideScorer,
        results: &[BoundaryBatchResult],
    ) -> Result<ValidatedRestoredAnalysis, Box<dyn Error>> {
        validate_restored_transcript(&sources, &restored)?;
        if restored.text() != self.text {
            return Err(BoundaryError::SourceChanged.into());
        }
        let ranges = self.partition(results)?;
        let scores = ranges
            .iter()
            .map(|range| scorer.score_slides(&self.text[range.clone()]))
            .collect::<Result<Vec<_>, _>>()?;
        let positions = infer_slide_positions(&scores)?;
        let passages = ranges
            .into_iter()
            .zip(positions)
            .map(|(range, slide_position)| {
                let (source_start, source_end) = source_provenance(&restored.spans, &range)
                    .ok_or(BoundaryError::SourceChanged)?;
                Ok(RestoredLecturePassage {
                    text: self.text[range].to_owned(),
                    source_start,
                    source_end,
                    slide_position,
                    novelty: Score5::ZERO,
                    importance: Score5::ZERO,
                    comparative_novelty: None,
                    comparative_importance: None,
                    related_slides: Vec::new(),
                    summary: None,
                    comparison_note: None,
                })
            })
            .collect::<Result<Vec<_>, BoundaryError>>()?;
        Ok(ValidatedRestoredAnalysis::new(sources, restored, passages)?)
    }
}

fn split_at(text: &str, base: usize, endings: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    let mut pieces = Vec::new();
    for (index, character) in text.char_indices() {
        if endings.contains(character) {
            let end = index + character.len_utf8();
            pieces.push(base + start..base + end);
            start = end;
        }
    }
    if start < text.len() {
        pieces.push(base + start..base + text.len());
    }
    pieces
}

fn atomize(text: &str) -> Vec<Range<usize>> {
    split_at(text, 0, "。！？!?；;\n")
        .into_iter()
        .flat_map(|range| {
            if text[range.clone()].chars().count() > LONG_ATOM_CHARACTERS {
                split_at(&text[range.clone()], range.start, "，,")
            } else {
                vec![range]
            }
        })
        .flat_map(|range| split_long_atom(text, range))
        .collect()
}

fn split_long_atom(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let character_count = text[range.clone()].chars().count();
    if character_count <= MAX_ATOM_CHARACTERS {
        return vec![range];
    }

    let part_count = character_count.div_ceil(MAX_ATOM_CHARACTERS);
    let short_part = character_count / part_count;
    let longer_parts = character_count % part_count;
    let mut boundaries = text[range.clone()]
        .char_indices()
        .map(|(offset, _)| range.start + offset);
    let mut start = boundaries.next().expect("a long atom is non-empty");
    let mut pieces = Vec::with_capacity(part_count);
    for part in 0..part_count {
        let length = short_part + usize::from(part < longer_parts);
        let end = if part + 1 == part_count {
            range.end
        } else {
            boundaries
                .nth(length - 1)
                .expect("the atom has enough character boundaries")
        };
        pieces.push(start..end);
        start = end;
    }
    pieces
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartitionCost {
    /// Counts are ordered from the most damaging cut cost (5) to 1. Cost-zero
    /// cuts do not need a bucket because passage count resolves their ties.
    damaging_cuts: [usize; 5],
    passage_count: usize,
    squared_lengths: u64,
}

impl PartitionCost {
    const ZERO: Self = Self {
        damaging_cuts: [0; 5],
        passage_count: 0,
        squared_lengths: 0,
    };

    fn append_passage(mut self, characters: usize, cut_cost: Option<Score5>) -> Self {
        if let Some(cost) = cut_cost
            && cost.get() > 0
        {
            self.damaging_cuts[(5 - cost.get()) as usize] += 1;
        }
        self.passage_count += 1;
        self.squared_lengths = self
            .squared_lengths
            .saturating_add((characters as u64).saturating_pow(2));
        self
    }

    fn is_better_than(self, other: Self) -> bool {
        self.damaging_cuts
            .cmp(&other.damaging_cuts)
            .then_with(|| self.passage_count.cmp(&other.passage_count))
            .then_with(|| self.squared_lengths.cmp(&other.squared_lengths))
            .is_lt()
    }
}

fn partition(
    text: &str,
    atoms: &[Range<usize>],
    cut_costs: &[Score5],
) -> Result<Vec<Range<usize>>, BoundaryError> {
    let mut lengths = vec![0];
    for atom in atoms {
        let count = text[atom.clone()].chars().count();
        lengths.push(lengths.last().unwrap() + count);
    }
    let mut best = vec![None; atoms.len() + 1];
    let mut previous = vec![None; atoms.len() + 1];
    best[0] = Some(PartitionCost::ZERO);
    for end in 1..=atoms.len() {
        for start in (0..end).rev() {
            let length = lengths[end] - lengths[start];
            if length > BOUNDARY_MAX_PASSAGE_CHARACTERS {
                break;
            }
            let Some(prefix) = best[start] else {
                continue;
            };
            let candidate =
                prefix.append_passage(length, (end < atoms.len()).then(|| cut_costs[end - 1]));
            if best[end].is_none_or(|current| candidate.is_better_than(current)) {
                best[end] = Some(candidate);
                previous[end] = Some(start);
            }
        }
    }
    let mut ranges = Vec::new();
    let mut end = atoms.len();
    while end > 0 {
        let start = previous[end].ok_or(BoundaryError::NoLegalPartition {
            max_characters: BOUNDARY_MAX_PASSAGE_CHARACTERS,
        })?;
        ranges.push(atoms[start].start..atoms[end - 1].end);
        end = start;
    }
    ranges.reverse();
    Ok(ranges)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundaryError {
    InvalidResponse(String),
    NoLegalPartition { max_characters: usize },
    SourceChanged,
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResponse(message) => write!(f, "invalid boundary response: {message}"),
            Self::NoLegalPartition { max_characters } => write!(
                f,
                "no exact source partition fits the {max_characters}-character passage limit"
            ),
            Self::SourceChanged => f.write_str("boundary plan does not match the restored source"),
        }
    }
}
impl Error for BoundaryError {}
