use std::{collections::HashSet, error::Error, fmt};

use crate::{SlideId, SlideScore};

const PREFERRED_TRANSITION_RADIUS: usize = 3;
const NEARBY_PENALTY_PER_SLIDE: f64 = 0.05;
const DISTANT_JUMP_PENALTY: f64 = 0.50;
const DISTANT_PENALTY_PER_SLIDE: f64 = 0.02;

/// Infers one slide position per score row for a complete lecture.
///
/// The path starts on the first slide, uses symmetric soft transition costs,
/// and can therefore move backward or make a sufficiently supported large jump.
pub fn infer_slide_positions(
    score_rows: &[Vec<SlideScore>],
) -> Result<Vec<SlideId>, SlideAlignmentError> {
    let Some(first_row) = score_rows.first() else {
        return Ok(Vec::new());
    };
    validate_score_rows(score_rows)?;

    let slide_ids: Vec<_> = first_row.iter().map(|score| score.slide_id).collect();
    let first_emissions = normalize(first_row);
    let mut totals = vec![f64::NEG_INFINITY; slide_ids.len()];
    totals[0] = first_emissions[0];
    let mut backpointers = vec![vec![0; slide_ids.len()]];

    for row in &score_rows[1..] {
        let emissions = normalize(row);
        let mut next_totals = vec![0.0; slide_ids.len()];
        let mut row_backpointers = vec![0; slide_ids.len()];

        for current in 0..slide_ids.len() {
            let (best_previous, best_total) = totals
                .iter()
                .copied()
                .enumerate()
                .map(|(previous, total)| (previous, total - transition_penalty(previous, current)))
                .max_by(|left, right| left.1.total_cmp(&right.1))
                .expect("totals is non-empty");
            next_totals[current] = best_total + emissions[current];
            row_backpointers[current] = best_previous;
        }

        totals = next_totals;
        backpointers.push(row_backpointers);
    }

    let final_position = totals
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .map(|(position, _)| position)
        .expect("totals is non-empty");

    let mut positions = vec![0; score_rows.len()];
    let mut position = final_position;
    for row in (0..score_rows.len()).rev() {
        positions[row] = position;
        if row > 0 {
            position = backpointers[row][position];
        }
    }

    Ok(positions
        .into_iter()
        .map(|position| slide_ids[position])
        .collect())
}

fn validate_score_rows(score_rows: &[Vec<SlideScore>]) -> Result<(), SlideAlignmentError> {
    let first_row = &score_rows[0];
    if first_row.is_empty() {
        return Err(SlideAlignmentError::EmptyScoreRow { row: 1 });
    }
    let expected_ids: Vec<_> = first_row.iter().map(|score| score.slide_id).collect();
    let mut unique_ids = HashSet::new();
    for score in first_row {
        if !unique_ids.insert(score.slide_id) {
            return Err(SlideAlignmentError::DuplicateSlide {
                row: 1,
                slide_id: score.slide_id,
            });
        }
    }

    for (position, row) in score_rows.iter().enumerate() {
        let row_number = position + 1;
        if row.is_empty() {
            return Err(SlideAlignmentError::EmptyScoreRow { row: row_number });
        }
        if row
            .iter()
            .map(|score| score.slide_id)
            .ne(expected_ids.iter().copied())
        {
            return Err(SlideAlignmentError::InconsistentSlides { row: row_number });
        }
        if let Some(score) = row.iter().find(|score| !score.score.is_finite()) {
            return Err(SlideAlignmentError::NonFiniteScore {
                row: row_number,
                slide_id: score.slide_id,
            });
        }
    }
    Ok(())
}

fn normalize(scores: &[SlideScore]) -> Vec<f64> {
    let minimum = scores
        .iter()
        .map(|score| score.score)
        .fold(f64::INFINITY, f64::min);
    let maximum = scores
        .iter()
        .map(|score| score.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = maximum - minimum;
    if range == 0.0 {
        vec![0.0; scores.len()]
    } else {
        scores
            .iter()
            .map(|score| (score.score - minimum) / range)
            .collect()
    }
}

fn transition_penalty(previous: usize, current: usize) -> f64 {
    let distance = previous.abs_diff(current);
    if distance <= PREFERRED_TRANSITION_RADIUS {
        distance as f64 * NEARBY_PENALTY_PER_SLIDE
    } else {
        DISTANT_JUMP_PENALTY
            + (distance - PREFERRED_TRANSITION_RADIUS - 1) as f64 * DISTANT_PENALTY_PER_SLIDE
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideAlignmentError {
    EmptyScoreRow { row: usize },
    DuplicateSlide { row: usize, slide_id: SlideId },
    InconsistentSlides { row: usize },
    NonFiniteScore { row: usize, slide_id: SlideId },
}

impl fmt::Display for SlideAlignmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScoreRow { row } => write!(formatter, "slide-score row {row} is empty"),
            Self::DuplicateSlide { row, slide_id } => write!(
                formatter,
                "slide-score row {row} repeats slide {}",
                slide_id.0
            ),
            Self::InconsistentSlides { row } => write!(
                formatter,
                "slide-score row {row} does not contain the same slides in presentation order"
            ),
            Self::NonFiniteScore { row, slide_id } => write!(
                formatter,
                "slide-score row {row} has a non-finite score for slide {}",
                slide_id.0
            ),
        }
    }
}

impl Error for SlideAlignmentError {}
