use beyond_slides::{
    LecturePassage, LecturePassages, Score5, SentenceId, Slide, SlideDeck, SlideId, Transcript,
    TranscriptSentence, ValidatedAnalysis, ValidatedSources,
};

#[test]
fn normalized_sources_can_be_validated_before_analysis() {
    let transcript = Transcript {
        sentences: vec![TranscriptSentence {
            id: SentenceId(10),
            start_ms: 0,
            end_ms: 1_000,
            text: "A valid transcript sentence.".to_owned(),
        }],
    };
    let slide_deck = SlideDeck {
        slides: vec![Slide {
            id: SlideId(20),
            text: "A valid slide.".to_owned(),
        }],
    };

    let sources = ValidatedSources::new(transcript, slide_deck)
        .expect("valid normalized sources should be accepted without lecture passages");

    assert_eq!(
        (
            sources.transcript().sentences[0].id,
            sources.slide_deck().slides[0].id,
        ),
        (SentenceId(10), SlideId(20))
    );
}

#[test]
fn validated_sources_can_be_completed_with_lecture_passages() {
    let sources = ValidatedSources::new(
        Transcript {
            sentences: vec![TranscriptSentence {
                id: SentenceId(10),
                start_ms: 0,
                end_ms: 1_000,
                text: "A valid transcript sentence.".to_owned(),
            }],
        },
        SlideDeck {
            slides: vec![Slide {
                id: SlideId(20),
                text: "A valid slide.".to_owned(),
            }],
        },
    )
    .expect("the normalized sources should be valid");
    let passages = LecturePassages {
        passages: vec![LecturePassage {
            start: SentenceId(10),
            end: SentenceId(10),
            novelty: score(3),
            connection_strength: score(0),
            importance: score(4),
            related_slides: vec![SlideId(20)],
            summary: None,
            comparison_note: None,
        }],
    };

    let analysis = ValidatedAnalysis::new(sources, passages)
        .expect("valid lecture passages should complete the analysis");

    assert_eq!(analysis.passages().len(), 1);
}

fn score(value: u8) -> Score5 {
    Score5::try_from(value).expect("test score should be valid")
}
