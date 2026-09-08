use beyond_slides::{LecturePassages, SlideDeck, Transcript, ValidatedAnalysis, ValidatedSources};

#[test]
fn tiny_course_crosses_the_json_and_validation_seams() {
    let transcript: Transcript =
        serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))
            .expect("the transcript fixture should match its public JSON format");
    let slide_deck: SlideDeck =
        serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))
            .expect("the slide fixture should match its public JSON format");
    let passages: LecturePassages =
        serde_json::from_str(include_str!("../examples/tiny_course/annotations.json"))
            .expect("the passage fixture should match its public JSON format");

    let sources = ValidatedSources::new(transcript, slide_deck)
        .expect("the tiny course sources should satisfy every source invariant");
    let analysis = ValidatedAnalysis::new(sources, passages)
        .expect("the tiny course should satisfy every analysis invariant");

    assert_eq!(analysis.passages().len(), 5);
}

#[test]
fn score_outside_zero_to_five_is_rejected_at_the_json_seam() {
    let passages = serde_json::from_str::<LecturePassages>(
        r#"
        {
          "passages": [{
            "start": 0,
            "end": 1,
            "novelty": 6,
            "importance": 0,
            "related_slides": [],
            "comparison_note": "Invalid only because novelty exceeds five."
          }]
        }
        "#,
    );

    assert!(passages.is_err());
}
