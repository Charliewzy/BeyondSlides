use beyond_slides::{
    LecturePassage, LecturePassages, Score5, Slide, SlideDeck, SlideId, Transcript,
    TranscriptSegment, TranscriptSegmentId, ValidatedAnalysis, ValidatedSources, ValidationError,
};

#[test]
fn noncanonical_transcript_segment_id_is_rejected() {
    let (mut transcript, slide_deck, _passages) = valid_analysis_parts();
    transcript.segments[1].id = TranscriptSegmentId(10);

    let error = ValidatedSources::new(transcript, slide_deck)
        .expect_err("a segment ID must equal its zero-based position");

    assert_eq!(
        error,
        ValidationError::NonCanonicalTranscriptSegmentId {
            position: 1,
            actual: TranscriptSegmentId(10),
        }
    );
}

#[test]
fn empty_transcript_segment_text_is_rejected() {
    let (mut transcript, slide_deck, _passages) = valid_analysis_parts();
    transcript.segments[1].text = "   ".to_owned();

    let error = ValidatedSources::new(transcript, slide_deck)
        .expect_err("empty transcript segment text must be rejected");

    assert_eq!(
        error,
        ValidationError::EmptyTranscriptSegment {
            id: TranscriptSegmentId(1)
        }
    );
}

#[test]
fn transcript_segment_ending_before_it_starts_is_rejected() {
    let (mut transcript, slide_deck, _passages) = valid_analysis_parts();
    transcript.segments[1].start_ms = Some(2_001);

    let error = ValidatedSources::new(transcript, slide_deck)
        .expect_err("a reversed timestamp range must be rejected");

    assert_eq!(
        error,
        ValidationError::InvalidTranscriptSegmentTimeRange {
            id: TranscriptSegmentId(1),
            start_ms: 2_001,
            end_ms: 2_000,
        }
    );
}

#[test]
fn transcript_segment_times_must_follow_presentation_order() {
    let (mut transcript, slide_deck, _passages) = valid_analysis_parts();
    transcript.segments[0].start_ms = Some(600);
    transcript.segments[1].start_ms = Some(500);

    let error = ValidatedSources::new(transcript, slide_deck)
        .expect_err("timestamps that move backward must be rejected");

    assert_eq!(
        error,
        ValidationError::TranscriptSegmentOutOfOrder {
            previous: TranscriptSegmentId(0),
            current: TranscriptSegmentId(1),
        }
    );
}

#[test]
fn noncanonical_slide_id_is_rejected() {
    let (transcript, mut slide_deck, _passages) = valid_analysis_parts();
    slide_deck.slides[1].id = SlideId(0);

    let error = ValidatedSources::new(transcript, slide_deck)
        .expect_err("a slide ID must equal its zero-based position");

    assert_eq!(
        error,
        ValidationError::NonCanonicalSlideId {
            position: 1,
            actual: SlideId(0),
        }
    );
}

#[test]
fn passage_related_slide_must_exist() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].related_slides.push(SlideId(99));

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("unknown related slides must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownRelatedSlide {
            passage_start: TranscriptSegmentId(1),
            slide: SlideId(99),
        }
    );
}

#[test]
fn passage_related_slides_cannot_repeat() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].related_slides.push(SlideId(0));

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("duplicate related slides must be rejected");

    assert_eq!(
        error,
        ValidationError::DuplicateRelatedSlide {
            passage_start: TranscriptSegmentId(1),
            slide: SlideId(0),
        }
    );
}

#[test]
fn passage_may_omit_a_comparison_note() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].comparison_note = None;

    let analysis = validate_analysis(transcript, slide_deck, passages);

    assert!(analysis.is_ok());
}

#[test]
fn oral_addition_may_omit_a_summary() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].summary = None;

    let analysis = validate_analysis(transcript, slide_deck, passages);

    assert!(analysis.is_ok());
}

#[test]
fn passage_start_must_reference_a_transcript_segment() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = TranscriptSegmentId(3);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("unknown passage starts must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownPassageStart {
            start: TranscriptSegmentId(3),
        }
    );
}

#[test]
fn passage_end_must_reference_a_transcript_segment() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].end = TranscriptSegmentId(3);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("unknown passage ends must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownPassageEnd {
            passage_start: TranscriptSegmentId(1),
            end: TranscriptSegmentId(3),
        }
    );
}

#[test]
fn passage_cannot_end_before_it_starts() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = TranscriptSegmentId(1);
    passages.passages[0].end = TranscriptSegmentId(0);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("reversed passage ranges must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageEndBeforeStart {
            start: TranscriptSegmentId(1),
            end: TranscriptSegmentId(0),
        }
    );
}

#[test]
fn first_passage_must_start_at_the_first_transcript_segment() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = TranscriptSegmentId(1);
    passages.passages[0].end = TranscriptSegmentId(1);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("an uncovered transcript beginning must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageCoverageMismatch {
            expected: TranscriptSegmentId(0),
            actual: TranscriptSegmentId(1),
        }
    );
}

#[test]
fn consecutive_passages_cannot_leave_a_gap() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].start = TranscriptSegmentId(2);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("a gap between lecture passages must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageCoverageMismatch {
            expected: TranscriptSegmentId(1),
            actual: TranscriptSegmentId(2),
        }
    );
}

#[test]
fn final_passage_must_reach_the_end_of_the_transcript() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].end = TranscriptSegmentId(1);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("an uncovered transcript ending must be rejected");

    assert_eq!(
        error,
        ValidationError::UncoveredTranscriptTail {
            expected: TranscriptSegmentId(2),
        }
    );
}

#[test]
fn nonempty_transcript_requires_at_least_one_passage() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages.clear();

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("a nonempty transcript cannot have no lecture passages");

    assert_eq!(
        error,
        ValidationError::UncoveredTranscriptTail {
            expected: TranscriptSegmentId(0),
        }
    );
}

#[test]
fn passage_after_the_transcript_is_already_covered_is_rejected() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].end = TranscriptSegmentId(2);

    let error = validate_analysis(transcript, slide_deck, passages)
        .expect_err("an extra overlapping passage must be rejected");

    assert_eq!(
        error,
        ValidationError::UnexpectedPassage {
            actual: TranscriptSegmentId(1),
        }
    );
}

fn validate_analysis(
    transcript: Transcript,
    slide_deck: SlideDeck,
    passages: LecturePassages,
) -> Result<ValidatedAnalysis, ValidationError> {
    let sources = ValidatedSources::new(transcript, slide_deck)
        .expect("the shared test sources should be valid");
    ValidatedAnalysis::new(sources, passages)
}

fn valid_analysis_parts() -> (Transcript, SlideDeck, LecturePassages) {
    let transcript = Transcript {
        segments: vec![
            segment(0, 0, 1_000, "The slide states the search procedure."),
            segment(1, 1_000, 2_000, "The lecturer adds an intuition."),
            segment(2, 2_000, 3_000, "The lecturer gives practical advice."),
        ],
    };
    let slide_deck = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(0),
                text: "Search procedure".to_owned(),
            },
            Slide {
                id: SlideId(1),
                text: "Complexity".to_owned(),
            },
        ],
    };
    let passages = LecturePassages {
        passages: vec![
            passage(0, 0, 0, 1, vec![0], None),
            passage(1, 2, 3, 5, vec![0, 1], Some("A useful oral addition")),
        ],
    };

    (transcript, slide_deck, passages)
}

fn segment(id: u32, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: TranscriptSegmentId(id),
        start_ms: Some(start_ms),
        end_ms: Some(end_ms),
        text: text.to_owned(),
    }
}

fn passage(
    start: u32,
    end: u32,
    novelty: u8,
    importance: u8,
    related_slides: Vec<u32>,
    summary: Option<&str>,
) -> LecturePassage {
    LecturePassage {
        start: TranscriptSegmentId(start),
        end: TranscriptSegmentId(end),
        novelty: score(novelty),
        importance: score(importance),
        related_slides: related_slides.into_iter().map(SlideId).collect(),
        summary: summary.map(str::to_owned),
        comparison_note: Some("A concise comparison note.".to_owned()),
    }
}

fn score(value: u8) -> Score5 {
    Score5::try_from(value).expect("test score should be in range")
}
