use std::{error::Error, path::Path};

use beyond_slides::{
    SlideId,
    evaluation::{align_video_to_slides, align_video_to_slides_with_progress},
};

#[test]
fn video_frames_are_matched_to_rendered_pdf_pages() -> Result<(), Box<dyn Error>> {
    let fixture_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    let alignment = align_video_to_slides(
        &fixture_directory.join("visual_alignment.mp4"),
        &fixture_directory.join("visual_alignment.pdf"),
    )?;

    assert_eq!(alignment.frame_width, 320);
    assert_eq!(alignment.frame_height, 180);
    assert_eq!(alignment.sample_period_ms, 1_000);
    assert_eq!(alignment.frame_matches.len(), 2);
    assert_eq!(alignment.frame_matches[0].timestamp_ms, 0);
    assert_eq!(alignment.frame_matches[0].best.slide_id, SlideId(0));
    assert_eq!(alignment.frame_matches[1].timestamp_ms, 1_000);
    assert_eq!(alignment.frame_matches[1].best.slide_id, SlideId(1));
    for frame_match in alignment.frame_matches {
        let runner_up = frame_match
            .runner_up
            .expect("the two-page fixture should have a runner-up");
        assert!(frame_match.best.score > runner_up.score);
    }
    Ok(())
}

#[test]
fn video_alignment_reports_processed_and_total_video_time() -> Result<(), Box<dyn Error>> {
    let fixture_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut updates = Vec::new();

    let alignment = align_video_to_slides_with_progress(
        &fixture_directory.join("visual_alignment.mp4"),
        &fixture_directory.join("visual_alignment.pdf"),
        |progress| updates.push(progress),
    )?;

    assert_eq!(alignment.frame_matches.len(), 2);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].processed_ms, 1_000);
    assert_eq!(updates[0].total_ms, 2_000);
    assert_eq!(updates[1].processed_ms, 2_000);
    assert_eq!(updates[1].total_ms, 2_000);
    Ok(())
}
