use beyond_slides::{
    LecturePassage, LecturePassages, Score5, SentenceId, Slide, SlideDeck, SlideId, Transcript,
    TranscriptSentence, ValidatedAnalysis, rank_oral_additions, render_report,
};

#[test]
fn report_presents_oral_additions_in_ranked_order() {
    let analysis = tiny_course();
    let ranked = rank_oral_additions(&analysis);

    let report = render_report(&analysis, &ranked);
    let ranked_starts: Vec<_> = [40, 130, 70]
        .map(|start| {
            report
                .find(&format!("data-ranked-passage-start=\"{start}\""))
                .expect("each qualifying oral addition should appear in the ranked view")
        })
        .into_iter()
        .collect();

    assert!(ranked_starts.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn ranked_oral_addition_carries_transcript_and_slide_evidence() {
    let analysis = tiny_course();
    let ranked = rank_oral_additions(&analysis);

    let report = render_report(&analysis, &ranked);

    assert!(
        [
            "00:11–00:23",
            "Each binary-search comparison can be understood as gaining one bit of information.",
            "A useful way to think about that comparison is as receiving one bit of information.",
            "Importance <strong>5</strong>",
            "Novelty <strong>3</strong>",
            "Connection <strong>2</strong>",
            "Slide 1",
            "Slide 4",
            "Binary search performs O(log n) comparisons.",
        ]
        .into_iter()
        .all(|evidence| report.contains(evidence))
    );
}

#[test]
fn report_preserves_every_passage_in_transcript_order() {
    let analysis = tiny_course();
    let ranked = rank_oral_additions(&analysis);

    let report = render_report(&analysis, &ranked);
    let transcript_starts: Vec<_> = [10, 40, 70, 100, 130]
        .map(|start| {
            report
                .find(&format!("data-transcript-passage-start=\"{start}\""))
                .expect("every lecture passage should appear in the transcript view")
        })
        .into_iter()
        .collect();

    assert!(transcript_starts.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn report_packages_its_presentation_in_the_html_document() {
    let analysis = tiny_course();
    let ranked = rank_oral_additions(&analysis);

    let report = render_report(&analysis, &ranked);

    assert!(
        report.contains("<style>")
            && !report.contains("href=\"http")
            && !report.contains("src=\"http")
    );
}

#[test]
fn transcript_evidence_becomes_the_title_when_summary_is_absent() {
    let transcript = Transcript {
        sentences: vec![TranscriptSentence {
            id: SentenceId(10),
            start_ms: 0,
            end_ms: 1_000,
            text: "<binary & search>".to_owned(),
        }],
    };
    let slide_deck = SlideDeck {
        slides: vec![Slide {
            id: SlideId(1),
            text: "Binary search".to_owned(),
        }],
    };
    let passages = LecturePassages {
        passages: vec![LecturePassage {
            start: SentenceId(10),
            end: SentenceId(10),
            novelty: Score5::try_from(3).expect("test score should be valid"),
            connection_strength: Score5::try_from(0).expect("test score should be valid"),
            importance: Score5::try_from(3).expect("test score should be valid"),
            related_slides: vec![SlideId(1)],
            summary: None,
            comparison_note: None,
        }],
    };
    let analysis = ValidatedAnalysis::new(transcript, slide_deck, passages)
        .expect("the test analysis should be valid");

    let ranked = rank_oral_additions(&analysis);
    let report = render_report(&analysis, &ranked);
    let title = report
        .split_once("<h3>")
        .and_then(|(_, remainder)| remainder.split_once("</h3>"))
        .map(|(title, _)| title)
        .expect("the oral addition should have a title");

    assert!(
        title.contains("binary")
            && title.contains("search")
            && !title.contains('<')
            && !title.contains('>')
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
