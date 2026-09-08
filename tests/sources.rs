use beyond_slides::{
    LecturePassage, LecturePassages, Score5, Slide, SlideDeck, SlideId, Transcript,
    TranscriptSegment, TranscriptSegmentId, ValidatedAnalysis, ValidatedSources,
};

#[test]
fn normalized_sources_can_be_validated_before_analysis() {
    let transcript = Transcript {
        segments: vec![TranscriptSegment {
            id: TranscriptSegmentId(0),
            start_ms: Some(0),
            end_ms: Some(1_000),
            text: "A valid transcript segment.".to_owned(),
        }],
    };
    let slide_deck = SlideDeck {
        slides: vec![Slide {
            id: SlideId(0),
            text: "A valid slide.".to_owned(),
        }],
    };

    let sources = ValidatedSources::new(transcript, slide_deck)
        .expect("valid normalized sources should be accepted without lecture passages");

    assert_eq!(
        (
            sources.transcript().segments[0].id,
            sources.slide_deck().slides[0].id,
        ),
        (TranscriptSegmentId(0), SlideId(0))
    );
}

#[test]
fn validated_sources_can_be_completed_with_lecture_passages() {
    let sources = ValidatedSources::new(
        Transcript {
            segments: vec![TranscriptSegment {
                id: TranscriptSegmentId(0),
                start_ms: Some(0),
                end_ms: Some(1_000),
                text: "A valid transcript segment.".to_owned(),
            }],
        },
        SlideDeck {
            slides: vec![Slide {
                id: SlideId(0),
                text: "A valid slide.".to_owned(),
            }],
        },
    )
    .expect("the normalized sources should be valid");
    let passages = LecturePassages {
        passages: vec![LecturePassage {
            start: TranscriptSegmentId(0),
            end: TranscriptSegmentId(0),
            novelty: score(3),
            importance: score(4),
            related_slides: vec![SlideId(0)],
            summary: None,
            comparison_note: None,
        }],
    };

    let analysis = ValidatedAnalysis::new(sources, passages)
        .expect("valid lecture passages should complete the analysis");

    assert_eq!(analysis.passages().len(), 1);
}

#[test]
fn legacy_transcript_json_with_sentences_still_deserializes() {
    let transcript: Transcript = serde_json::from_str(
        r#"{
            "sentences": [
                { "id": 0, "start_ms": 0, "end_ms": 1000, "text": "甲" }
            ]
        }"#,
    )
    .expect("the pre-rename transcript field should remain readable");

    assert_eq!(
        transcript.segments,
        vec![TranscriptSegment {
            id: TranscriptSegmentId(0),
            start_ms: Some(0),
            end_ms: Some(1_000),
            text: "甲".into(),
        }]
    );
}

fn score(value: u8) -> Score5 {
    Score5::try_from(value).expect("test score should be valid")
}
