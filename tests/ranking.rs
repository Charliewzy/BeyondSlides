use beyond_slides::{
    LecturePassages, SentenceId, SlideDeck, Transcript, ValidatedAnalysis, rank_oral_additions,
};

#[test]
fn tiny_course_oral_additions_are_filtered_and_ranked() {
    let analysis = tiny_course();

    let ranked_starts: Vec<SentenceId> = rank_oral_additions(&analysis)
        .into_iter()
        .map(|passage| passage.start)
        .collect();

    assert_eq!(
        ranked_starts,
        vec![SentenceId(40), SentenceId(130), SentenceId(70)]
    );
}

fn tiny_course() -> ValidatedAnalysis {
    let transcript: Transcript =
        serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))
            .expect("the transcript fixture should match its public JSON format");
    let slide_deck: SlideDeck =
        serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))
            .expect("the slide fixture should match its public JSON format");
    let passages: LecturePassages =
        serde_json::from_str(include_str!("../examples/tiny_course/annotations.json"))
            .expect("the passage fixture should match its public JSON format");

    ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect("the tiny course should satisfy every analysis invariant")
}
