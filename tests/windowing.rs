use std::time::Duration;

use beyond_slides::{
    SlideDeck, Transcript, TranscriptSegment, TranscriptSegmentId, ValidatedSources,
    WindowingConfig, build_windows,
};

#[test]
fn owned_regions_respect_character_budget_without_splitting_segments() {
    let sources = sources(&[
        ("甲乙丙", 0, 1_000),
        ("丁戊", 1_000, 2_000),
        ("己庚辛壬癸甲", 2_000, 3_000),
        ("乙丙", 3_000, 4_000),
    ]);
    let config = WindowingConfig::new(5, Duration::from_secs(60), 0)
        .expect("nonzero character and duration budgets should be valid");

    let owned_regions: Vec<_> = build_windows(&sources, config)
        .iter()
        .map(|window| segment_ids(window.owned_region()))
        .collect();

    assert_eq!(owned_regions, vec![vec![0, 1], vec![2], vec![3]]);
}

#[test]
fn owned_regions_stop_before_exceeding_the_duration_budget() {
    let sources = sources(&[
        ("甲", 0, 10_000),
        ("乙", 10_000, 20_000),
        ("丙", 90_000, 91_000),
        ("丁", 91_000, 92_000),
    ]);
    let config = WindowingConfig::new(100, Duration::from_secs(60), 0)
        .expect("nonzero character and duration budgets should be valid");

    let owned_regions: Vec<_> = build_windows(&sources, config)
        .iter()
        .map(|window| segment_ids(window.owned_region()))
        .collect();

    assert_eq!(owned_regions, vec![vec![0, 1], vec![2, 3]]);
}

#[test]
fn context_uses_nearest_complete_segments_within_its_character_budget() {
    let sources = sources(&[
        ("甲乙", 0, 1_000),
        ("丙丁", 1_000, 2_000),
        ("戊己", 2_000, 3_000),
        ("庚辛", 3_000, 4_000),
        ("壬癸", 4_000, 5_000),
        ("子丑", 5_000, 6_000),
    ]);
    let config = WindowingConfig::new(4, Duration::from_secs(60), 3)
        .expect("nonzero character and duration budgets should be valid");

    let visible_regions: Vec<_> = build_windows(&sources, config)
        .iter()
        .map(|window| {
            (
                segment_ids(window.left_context()),
                segment_ids(window.owned_region()),
                segment_ids(window.right_context()),
            )
        })
        .collect();

    assert_eq!(
        visible_regions,
        vec![
            (vec![], vec![0, 1], vec![2]),
            (vec![1], vec![2, 3], vec![4]),
            (vec![3], vec![4, 5], vec![]),
        ]
    );
}

fn segment_ids(segments: &[TranscriptSegment]) -> Vec<u32> {
    segments.iter().map(|segment| segment.id.0).collect()
}

fn sources(segments: &[(&str, u64, u64)]) -> ValidatedSources {
    let segments = segments
        .iter()
        .enumerate()
        .map(|(position, &(text, start_ms, end_ms))| TranscriptSegment {
            id: TranscriptSegmentId(
                u32::try_from(position).expect("the test fixture should fit in a u32"),
            ),
            start_ms: Some(start_ms),
            end_ms: Some(end_ms),
            text: text.to_owned(),
        })
        .collect();

    ValidatedSources::new(Transcript { segments }, SlideDeck { slides: vec![] })
        .expect("the test sources should be valid")
}
