use beyond_slides::{
    LecturePassage, LecturePassages, Score5, SentenceId, Slide, SlideDeck, SlideId, Transcript,
    TranscriptSentence, ValidatedAnalysis, ValidationError,
};

#[test]
fn duplicate_transcript_sentence_id_is_rejected() {
    let (mut transcript, slide_deck, passages) = valid_analysis_parts();
    transcript.sentences[1].id = SentenceId(10);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("duplicate sentence IDs must be rejected");

    assert_eq!(
        error,
        ValidationError::DuplicateSentenceId { id: SentenceId(10) }
    );
}

#[test]
fn empty_transcript_sentence_text_is_rejected() {
    let (mut transcript, slide_deck, passages) = valid_analysis_parts();
    transcript.sentences[1].text = "   ".to_owned();

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("empty transcript sentence text must be rejected");

    assert_eq!(
        error,
        ValidationError::EmptyTranscriptSentence { id: SentenceId(20) }
    );
}

#[test]
fn transcript_sentence_ending_before_it_starts_is_rejected() {
    let (mut transcript, slide_deck, passages) = valid_analysis_parts();
    transcript.sentences[1].start_ms = 2_001;

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("a reversed timestamp range must be rejected");

    assert_eq!(
        error,
        ValidationError::InvalidTranscriptSentenceTimeRange {
            id: SentenceId(20),
            start_ms: 2_001,
            end_ms: 2_000,
        }
    );
}

#[test]
fn transcript_sentence_times_must_follow_presentation_order() {
    let (mut transcript, slide_deck, passages) = valid_analysis_parts();
    transcript.sentences[0].start_ms = 600;
    transcript.sentences[1].start_ms = 500;

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("timestamps that move backward must be rejected");

    assert_eq!(
        error,
        ValidationError::TranscriptSentenceOutOfOrder {
            previous: SentenceId(10),
            current: SentenceId(20),
        }
    );
}

#[test]
fn duplicate_slide_id_is_rejected() {
    let (transcript, mut slide_deck, passages) = valid_analysis_parts();
    slide_deck.slides[1].id = SlideId(1);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("duplicate slide IDs must be rejected");

    assert_eq!(error, ValidationError::DuplicateSlideId { id: SlideId(1) });
}

#[test]
fn passage_related_slide_must_exist() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].related_slides.push(SlideId(99));

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("unknown related slides must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownRelatedSlide {
            passage_start: SentenceId(20),
            slide: SlideId(99),
        }
    );
}

#[test]
fn passage_related_slides_cannot_repeat() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].related_slides.push(SlideId(1));

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("duplicate related slides must be rejected");

    assert_eq!(
        error,
        ValidationError::DuplicateRelatedSlide {
            passage_start: SentenceId(20),
            slide: SlideId(1),
        }
    );
}

#[test]
fn passage_may_omit_a_comparison_note() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].comparison_note = None;

    let analysis = ValidatedAnalysis::new(transcript, slide_deck, passages);

    assert!(analysis.is_ok());
}

#[test]
fn oral_addition_may_omit_a_summary() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].summary = None;

    let analysis = ValidatedAnalysis::new(transcript, slide_deck, passages);

    assert!(analysis.is_ok());
}

#[test]
fn passage_start_must_reference_a_transcript_sentence() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = SentenceId(11);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("unknown passage starts must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownPassageStart {
            start: SentenceId(11),
        }
    );
}

#[test]
fn passage_end_must_reference_a_transcript_sentence() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].end = SentenceId(31);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("unknown passage ends must be rejected");

    assert_eq!(
        error,
        ValidationError::UnknownPassageEnd {
            passage_start: SentenceId(20),
            end: SentenceId(31),
        }
    );
}

#[test]
fn passage_cannot_end_before_it_starts() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = SentenceId(20);
    passages.passages[0].end = SentenceId(10);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("reversed passage ranges must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageEndBeforeStart {
            start: SentenceId(20),
            end: SentenceId(10),
        }
    );
}

#[test]
fn first_passage_must_start_at_the_first_transcript_sentence() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].start = SentenceId(20);
    passages.passages[0].end = SentenceId(20);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("an uncovered transcript beginning must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageCoverageMismatch {
            expected: SentenceId(10),
            actual: SentenceId(20),
        }
    );
}

#[test]
fn consecutive_passages_cannot_leave_a_gap() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].start = SentenceId(30);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("a gap between lecture passages must be rejected");

    assert_eq!(
        error,
        ValidationError::PassageCoverageMismatch {
            expected: SentenceId(20),
            actual: SentenceId(30),
        }
    );
}

#[test]
fn final_passage_must_reach_the_end_of_the_transcript() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[1].end = SentenceId(20);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("an uncovered transcript ending must be rejected");

    assert_eq!(
        error,
        ValidationError::UncoveredTranscriptTail {
            expected: SentenceId(30),
        }
    );
}

#[test]
fn nonempty_transcript_requires_at_least_one_passage() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages.clear();

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("a nonempty transcript cannot have no lecture passages");

    assert_eq!(
        error,
        ValidationError::UncoveredTranscriptTail {
            expected: SentenceId(10),
        }
    );
}

#[test]
fn passage_after_the_transcript_is_already_covered_is_rejected() {
    let (transcript, slide_deck, mut passages) = valid_analysis_parts();
    passages.passages[0].end = SentenceId(30);

    let error = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect_err("an extra overlapping passage must be rejected");

    assert_eq!(
        error,
        ValidationError::UnexpectedPassage {
            actual: SentenceId(20),
        }
    );
}

fn valid_analysis_parts() -> (Transcript, SlideDeck, LecturePassages) {
    let transcript = Transcript {
        sentences: vec![
            sentence(10, 0, 1_000, "The slide states the search procedure."),
            sentence(20, 1_000, 2_000, "The lecturer adds an intuition."),
            sentence(30, 2_000, 3_000, "The lecturer gives practical advice."),
        ],
    };
    let slide_deck = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(1),
                text: "Search procedure".to_owned(),
            },
            Slide {
                id: SlideId(2),
                text: "Complexity".to_owned(),
            },
        ],
    };
    let passages = LecturePassages {
        passages: vec![
            passage(10, 10, 0, 0, 1, vec![1], None),
            passage(20, 30, 3, 2, 5, vec![1, 2], Some("A useful oral addition")),
        ],
    };

    (transcript, slide_deck, passages)
}

fn sentence(id: u32, start_ms: u64, end_ms: u64, text: &str) -> TranscriptSentence {
    TranscriptSentence {
        id: SentenceId(id),
        start_ms,
        end_ms,
        text: text.to_owned(),
    }
}

#[allow(clippy::too_many_arguments)]
fn passage(
    start: u32,
    end: u32,
    novelty: u8,
    connection_strength: u8,
    importance: u8,
    related_slides: Vec<u32>,
    summary: Option<&str>,
) -> LecturePassage {
    LecturePassage {
        start: SentenceId(start),
        end: SentenceId(end),
        novelty: score(novelty),
        connection_strength: score(connection_strength),
        importance: score(importance),
        related_slides: related_slides.into_iter().map(SlideId).collect(),
        summary: summary.map(str::to_owned),
        comparison_note: Some("A concise comparison note.".to_owned()),
    }
}

fn score(value: u8) -> Score5 {
    Score5::try_from(value).expect("test score should be in range")
}
