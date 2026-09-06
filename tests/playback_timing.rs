use std::error::Error;

use beyond_slides::{
    PassagePlaybackInterval, PlaybackTimingBasis, PlaybackTimingError, RestoredLecturePassage,
    Score5, SlideId, TimedTranscript, TimedTranscriptToken, Transcript, TranscriptSegment,
    TranscriptSegmentId, project_passage_playback_intervals,
};

#[test]
fn overlapping_provenance_becomes_shared_precise_playback_boundaries() -> Result<(), Box<dyn Error>>
{
    let transcript = transcript("通用啊对吧你只要", 0, 800);
    let passages = vec![passage("通用，对吧。"), passage("你只要")];
    let timing = timed_characters("通用啊对吧你只要", 0, 100);

    let intervals = project_passage_playback_intervals(&transcript, &passages, &timing)?;

    assert_eq!(intervals.len(), 2);
    assert_eq!(intervals[0].basis, PlaybackTimingBasis::TimedTokens);
    assert_eq!(intervals[1].basis, PlaybackTimingBasis::TimedTokens);
    assert_eq!(intervals[0].end_ms, intervals[1].start_ms);
    assert_eq!(intervals[0].end_ms, 500);
    assert_eq!(intervals[1].end_ms, 800);
    Ok(())
}

#[test]
fn overlapping_provenance_is_aligned_as_one_ordered_passage_group() -> Result<(), Box<dyn Error>> {
    let transcript = transcript("甲对吧乙丙对吧", 0, 700);
    let passages = vec![passage("甲，对吧。"), passage("乙丙，对吧。")];
    let timing = timed_characters("甲对吧乙丙对吧", 0, 100);

    let intervals = project_passage_playback_intervals(&transcript, &passages, &timing)?;

    assert_eq!(
        intervals,
        vec![
            PassagePlaybackInterval {
                start_ms: 0,
                end_ms: 300,
                basis: PlaybackTimingBasis::TimedTokens,
            },
            PassagePlaybackInterval {
                start_ms: 300,
                end_ms: 700,
                basis: PlaybackTimingBasis::TimedTokens,
            },
        ]
    );
    Ok(())
}

#[test]
fn unrelated_timing_falls_back_to_the_source_segment_envelope() -> Result<(), Box<dyn Error>> {
    let transcript = transcript("原始内容", 120, 920);
    let timing = timed_characters("完全无关", 120, 200);

    let intervals =
        project_passage_playback_intervals(&transcript, &[passage("可读讲稿")], &timing)?;

    assert_eq!(
        intervals,
        vec![PassagePlaybackInterval {
            start_ms: 120,
            end_ms: 920,
            basis: PlaybackTimingBasis::TranscriptSegments,
        }]
    );
    Ok(())
}

#[test]
fn overlapping_timed_tokens_are_rejected() {
    let transcript = transcript("内容", 0, 300);
    let timing = TimedTranscript {
        tokens: vec![
            TimedTranscriptToken {
                text: "内".into(),
                start_ms: 0,
                end_ms: 200,
            },
            TimedTranscriptToken {
                text: "容".into(),
                start_ms: 100,
                end_ms: 300,
            },
        ],
    };

    let error = project_passage_playback_intervals(&transcript, &[passage("内容")], &timing)
        .expect_err("overlapping token timing must not be accepted");

    assert_eq!(
        error,
        PlaybackTimingError::OverlappingTimedTokens {
            token_index: 1,
            previous_end_ms: 200,
            start_ms: 100,
        }
    );
}

fn transcript(text: &str, start_ms: u64, end_ms: u64) -> Transcript {
    Transcript {
        segments: vec![TranscriptSegment {
            id: TranscriptSegmentId(0),
            start_ms,
            end_ms,
            text: text.into(),
        }],
    }
}

fn timed_characters(text: &str, start_ms: u64, duration_ms: u64) -> TimedTranscript {
    TimedTranscript {
        tokens: text
            .chars()
            .enumerate()
            .map(|(index, character)| TimedTranscriptToken {
                text: character.to_string(),
                start_ms: start_ms + index as u64 * duration_ms,
                end_ms: start_ms + (index as u64 + 1) * duration_ms,
            })
            .collect(),
    }
}

fn passage(text: &str) -> RestoredLecturePassage {
    RestoredLecturePassage {
        text: text.into(),
        source_start: TranscriptSegmentId(0),
        source_end: TranscriptSegmentId(0),
        slide_position: SlideId(0),
        novelty: score(0),
        connection_strength: score(0),
        importance: score(0),
        comparative_novelty: None,
        comparative_importance: None,
        related_slides: Vec::new(),
        summary: None,
        comparison_note: None,
    }
}

fn score(value: u8) -> Score5 {
    Score5::try_from(value).expect("test score is valid")
}
