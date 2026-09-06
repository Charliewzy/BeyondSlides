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
const PREFERRED_MIN: usize = 80;
const PREFERRED_MAX: usize = 280;
const HARD_MAX: usize = 450;

pub const PASSAGE_BOUNDARY_INSTRUCTIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/prompts/passage_boundaries.md"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryStrength {
    Continue,
    PossibleBreak,
    PreferredBreak,
    RequiredBreak,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryDecision {
    pub after_atom: usize,
    pub strength: BoundaryStrength,
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

/// Punctuation supplies possible cuts; model judgments and global DP select them.
/// The plan retains the exact source bytes, including spaces and omissions.
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
        let mut strengths = vec![BoundaryStrength::Continue; self.atoms.len().saturating_sub(1)];
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
                    strengths[boundary.after_atom] = boundary.strength;
                }
            }
        }
        partition(&self.text, &self.atoms, &strengths)
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
                    connection_strength: Score5::ZERO,
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
        .collect()
}

fn partition(
    text: &str,
    atoms: &[Range<usize>],
    strengths: &[BoundaryStrength],
) -> Result<Vec<Range<usize>>, BoundaryError> {
    let mut lengths = vec![0];
    for (index, atom) in atoms.iter().enumerate() {
        let count = text[atom.clone()].chars().count();
        if count > HARD_MAX {
            return Err(BoundaryError::Unbreakable {
                start_atom: index,
                characters: count,
            });
        }
        lengths.push(lengths.last().unwrap() + count);
    }
    let mut best = vec![f64::NEG_INFINITY; atoms.len() + 1];
    let mut previous = vec![None; atoms.len() + 1];
    best[0] = 0.0;
    for end in 1..=atoms.len() {
        if end < atoms.len() && strengths[end - 1] == BoundaryStrength::Continue {
            continue;
        }
        for start in (0..end).rev() {
            // A required break cannot be crossed, including by a shorter or
            // higher-scoring alternative. It is not merely a reward.
            if start + 1 < end && strengths[start] == BoundaryStrength::RequiredBreak {
                break;
            }
            let length = lengths[end] - lengths[start];
            if length > HARD_MAX {
                break;
            }
            let length_penalty = if length < PREFERRED_MIN {
                (PREFERRED_MIN - length) as f64 / 8.0
            } else if length > PREFERRED_MAX {
                (length - PREFERRED_MAX) as f64 / 12.0
            } else {
                0.0
            };
            let reward = if end == atoms.len() {
                0.0
            } else {
                match strengths[end - 1] {
                    BoundaryStrength::Continue => unreachable!("continue endpoints were skipped"),
                    BoundaryStrength::PossibleBreak => -2.0,
                    BoundaryStrength::PreferredBreak => 4.0,
                    BoundaryStrength::RequiredBreak => 16.0,
                }
            };
            let score = best[start] - length_penalty + reward;
            if score > best[end] {
                best[end] = score;
                previous[end] = Some(start);
            }
        }
    }
    let mut ranges = Vec::new();
    let mut end = atoms.len();
    while end > 0 {
        let start = previous[end].ok_or(BoundaryError::NoLegalPartition {
            max_characters: HARD_MAX,
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
    Unbreakable {
        start_atom: usize,
        characters: usize,
    },
    NoLegalPartition {
        max_characters: usize,
    },
    SourceChanged,
}

impl fmt::Display for BoundaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResponse(message) => write!(f, "invalid boundary response: {message}"),
            Self::Unbreakable {
                start_atom,
                characters,
            } => write!(
                f,
                "atom {start_atom} has {characters} characters and cannot fit the passage limit"
            ),
            Self::NoLegalPartition { max_characters } => write!(
                f,
                "no semantic partition fits {max_characters} characters without cutting a continue boundary; classifications are preserved for inspection"
            ),
            Self::SourceChanged => f.write_str("boundary plan does not match the restored source"),
        }
    }
}
impl Error for BoundaryError {}
