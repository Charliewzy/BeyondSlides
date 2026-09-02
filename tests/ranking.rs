use beyond_slides::{
    LecturePassages, SlideDeck, Transcript, TranscriptSegmentId, ValidatedAnalysis,
    ValidatedSources, rank_oral_additions,
};

#[test]
fn tiny_course_oral_additions_are_filtered_and_ranked() {
    let analysis = tiny_course();

    let ranked_starts: Vec<TranscriptSegmentId> = rank_oral_additions(&analysis)
        .into_iter()
        .map(|passage| passage.start)
        .collect();

    assert_eq!(
        ranked_starts,
        vec![
            TranscriptSegmentId(3),
            TranscriptSegmentId(12),
            TranscriptSegmentId(6)
        ]
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

    let sources = ValidatedSources::new(transcript, slide_deck)
        .expect("the tiny course sources should satisfy every source invariant");
    ValidatedAnalysis::new(sources, passages)
        .expect("the tiny course should satisfy every analysis invariant")
}
