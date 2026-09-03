use std::{error::Error, time::Duration};

use beyond_slides::{
    RestoredTranscript, RestoredTranscriptSpan, RestoredWindowingError, SlideDeck, Transcript,
    TranscriptSegment, TranscriptSegmentId, ValidatedSources, WindowingConfig,
    build_restored_windows,
};

#[test]
fn owned_regions_budget_restored_text_without_splitting_spans() -> Result<(), Box<dyn Error>> {
    let sources = sources(6);
    let restored = RestoredTranscript {
        spans: vec![
            text_span(0, 1, "甲乙丙"),
            omitted_span(2, 2),
            text_span(3, 4, "丁戊"),
            text_span(5, 5, "己庚辛壬癸甲"),
        ],
    };

    let windows = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(5, Duration::from_secs(60), 0)?,
    )?;

    assert_eq!(
        windows
            .iter()
            .map(|window| source_ranges(window.owned_region()))
            .collect::<Vec<_>>(),
        vec![vec![(0, 1), (2, 2), (3, 4)], vec![(5, 5)]]
    );
    assert_eq!(windows[0].owned_text(), "甲乙丙丁戊");
    assert_eq!(windows[1].owned_text(), "己庚辛壬癸甲");
    Ok(())
}

#[test]
fn owned_regions_measure_duration_through_raw_source_segments() -> Result<(), Box<dyn Error>> {
    let sources = timed_sources(&[(0, 10_000), (10_000, 20_000), (90_000, 91_000)]);
    let restored = RestoredTranscript {
        spans: vec![
            text_span(0, 0, "甲"),
            omitted_span(1, 1),
            text_span(2, 2, "乙"),
        ],
    };

    let windows = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(100, Duration::from_secs(60), 0)?,
    )?;

    assert_eq!(
        windows
            .iter()
            .map(|window| source_ranges(window.owned_region()))
            .collect::<Vec<_>>(),
        vec![vec![(0, 0), (1, 1)], vec![(2, 2)]]
    );
    Ok(())
}

#[test]
fn context_contains_nearest_complete_restored_spans() -> Result<(), Box<dyn Error>> {
    let sources = sources(5);
    let restored = RestoredTranscript {
        spans: vec![
            text_span(0, 0, "甲乙"),
            omitted_span(1, 1),
            text_span(2, 2, "丙丁"),
            text_span(3, 3, "戊己"),
            text_span(4, 4, "庚辛"),
        ],
    };

    let windows = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(2, Duration::from_secs(60), 2)?,
    )?;

    assert_eq!(windows.len(), 4);
    assert_eq!(source_ranges(windows[0].right_context()), vec![(2, 2)]);
    assert_eq!(
        source_ranges(windows[1].left_context()),
        vec![(0, 0), (1, 1)]
    );
    assert_eq!(source_ranges(windows[1].right_context()), vec![(3, 3)]);
    Ok(())
}

#[test]
fn leading_omissions_stay_with_the_first_readable_span_even_when_it_is_oversized()
-> Result<(), Box<dyn Error>> {
    let sources = sources(3);
    let restored = RestoredTranscript {
        spans: vec![omitted_span(0, 0), text_span(1, 2, "甲乙丙丁戊己")],
    };

    let windows = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(2, Duration::from_secs(60), 0)?,
    )?;

    assert_eq!(windows.len(), 1);
    assert_eq!(
        source_ranges(windows[0].owned_region()),
        vec![(0, 0), (1, 2)]
    );
    Ok(())
}

#[test]
fn an_all_disfluency_transcript_has_no_annotation_windows() -> Result<(), Box<dyn Error>> {
    let sources = sources(3);
    let restored = RestoredTranscript {
        spans: vec![omitted_span(0, 0), omitted_span(1, 2)],
    };

    let windows = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(100, Duration::from_secs(60), 0)?,
    )?;

    assert!(windows.is_empty());
    Ok(())
}

#[test]
fn invalid_restored_provenance_is_rejected_before_windowing() {
    let sources = sources(3);
    let restored = RestoredTranscript {
        spans: vec![text_span(0, 0, "甲"), text_span(2, 2, "丙")],
    };

    let error = build_restored_windows(
        &sources,
        &restored,
        WindowingConfig::new(100, Duration::from_secs(60), 0).expect("valid budget"),
    )
    .expect_err("a restored transcript cannot skip source evidence");

    assert_eq!(
        error,
        RestoredWindowingError::CoverageMismatch {
            span_index: 1,
            expected: TranscriptSegmentId(1),
            actual: TranscriptSegmentId(2),
        }
    );
}

fn sources(segment_count: usize) -> ValidatedSources {
    let times: Vec<_> = (0..segment_count)
        .map(|index| (index as u64 * 1_000, (index as u64 + 1) * 1_000))
        .collect();
    timed_sources(&times)
}

fn timed_sources(times: &[(u64, u64)]) -> ValidatedSources {
    let segments = times
        .iter()
        .enumerate()
        .map(|(index, &(start_ms, end_ms))| TranscriptSegment {
            id: TranscriptSegmentId(u32::try_from(index).expect("small fixture")),
            start_ms,
            end_ms,
            text: format!("原文{index}"),
        })
        .collect();
    ValidatedSources::new(Transcript { segments }, SlideDeck { slides: vec![] })
        .expect("valid fixture")
}

fn text_span(source_start: u32, source_end: u32, text: &str) -> RestoredTranscriptSpan {
    RestoredTranscriptSpan::Text {
        source_start: TranscriptSegmentId(source_start),
        source_end: TranscriptSegmentId(source_end),
        text: text.into(),
    }
}

fn omitted_span(source_start: u32, source_end: u32) -> RestoredTranscriptSpan {
    RestoredTranscriptSpan::OmittedDisfluency {
        source_start: TranscriptSegmentId(source_start),
        source_end: TranscriptSegmentId(source_end),
    }
}

fn source_ranges(spans: &[RestoredTranscriptSpan]) -> Vec<(u32, u32)> {
    spans
        .iter()
        .map(|span| (span.source_start().0, span.source_end().0))
        .collect()
}
