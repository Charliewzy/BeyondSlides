use std::error::Error;

use beyond_slides::{SlideId, SlideScore, infer_slide_positions};

fn row(scores: &[f64]) -> Vec<SlideScore> {
    scores
        .iter()
        .enumerate()
        .map(|(position, &score)| SlideScore {
            slide_id: SlideId(position as u32),
            score,
        })
        .collect()
}

#[test]
fn sustained_evidence_can_overcome_the_distant_jump_penalty() -> Result<(), Box<dyn Error>> {
    let score_rows = vec![
        row(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        row(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
        row(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
    ];

    let positions = infer_slide_positions(&score_rows)?;

    assert_eq!(positions, vec![SlideId(0), SlideId(7), SlideId(7)]);
    Ok(())
}

#[test]
fn a_complete_lecture_alignment_starts_at_the_first_slide() -> Result<(), Box<dyn Error>> {
    let score_rows = vec![row(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0])];

    let positions = infer_slide_positions(&score_rows)?;

    assert_eq!(positions, vec![SlideId(0)]);
    Ok(())
}

#[test]
fn score_rows_require_canonical_slide_ids() {
    let mut score_rows = vec![row(&[0.4, 0.6])];
    score_rows[0][0].slide_id = SlideId(1);

    let error = infer_slide_positions(&score_rows)
        .expect_err("slide-score IDs must equal their zero-based positions");

    assert_eq!(
        error,
        beyond_slides::SlideAlignmentError::InconsistentSlides { row: 1 }
    );
}

#[test]
fn slide_positions_ignore_an_isolated_distant_spike_and_allow_moving_backward()
-> Result<(), Box<dyn Error>> {
    let score_rows = vec![
        row(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        row(&[0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 1.0]),
        row(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        row(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ];

    let positions = infer_slide_positions(&score_rows)?;

    assert_eq!(
        positions,
        vec![SlideId(0), SlideId(3), SlideId(3), SlideId(2)]
    );
    Ok(())
}
