use beyond_slides::{
    SentenceId, SlideDeck, Transcript, ValidatedSources, WindowingConfig, build_windows,
};

#[test]
fn owned_regions_partition_the_transcript_in_presentation_order() {
    let sources = tiny_course_sources();
    let config = WindowingConfig::new(4, 2).expect("four owned sentences should be valid");

    let owned_regions: Vec<Vec<SentenceId>> = build_windows(&sources, config)
        .iter()
        .map(|window| {
            window
                .owned_region()
                .iter()
                .map(|sentence| sentence.id)
                .collect()
        })
        .collect();

    assert_eq!(
        owned_regions,
        vec![
            vec![
                SentenceId(10),
                SentenceId(20),
                SentenceId(30),
                SentenceId(40)
            ],
            vec![
                SentenceId(50),
                SentenceId(60),
                SentenceId(70),
                SentenceId(80)
            ],
            vec![
                SentenceId(90),
                SentenceId(100),
                SentenceId(110),
                SentenceId(120)
            ],
            vec![SentenceId(130), SentenceId(140), SentenceId(150)],
        ]
    );
}

#[test]
fn transcript_windows_add_context_without_changing_ownership() {
    let sources = tiny_course_sources();
    let config = WindowingConfig::new(4, 2).expect("four owned sentences should be valid");

    let visible_regions: Vec<_> = build_windows(&sources, config)
        .iter()
        .map(|window| {
            (
                sentence_ids(window.left_context()),
                sentence_ids(window.owned_region()),
                sentence_ids(window.right_context()),
            )
        })
        .collect();

    assert_eq!(
        visible_regions,
        vec![
            (vec![], vec![10, 20, 30, 40], vec![50, 60],),
            (vec![30, 40], vec![50, 60, 70, 80], vec![90, 100],),
            (vec![70, 80], vec![90, 100, 110, 120], vec![130, 140],),
            (vec![110, 120], vec![130, 140, 150], vec![],),
        ]
    );
}

#[test]
fn window_sizes_larger_than_the_transcript_are_clamped_to_its_edges() {
    let sources = tiny_course_sources();
    let config = WindowingConfig::new(usize::MAX, usize::MAX)
        .expect("a very large owned region should still be valid");

    let windows = build_windows(&sources, config);

    assert_eq!(
        windows
            .iter()
            .map(|window| {
                (
                    window.left_context().len(),
                    window.owned_region().len(),
                    window.right_context().len(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(0, 15, 0)]
    );
}

fn sentence_ids(sentences: &[beyond_slides::TranscriptSentence]) -> Vec<u32> {
    sentences.iter().map(|sentence| sentence.id.0).collect()
}

fn tiny_course_sources() -> ValidatedSources {
    let transcript: Transcript =
        serde_json::from_str(include_str!("../examples/tiny_course/transcript.json"))
            .expect("the transcript fixture should match its public JSON format");
    let slide_deck: SlideDeck =
        serde_json::from_str(include_str!("../examples/tiny_course/slides.json"))
            .expect("the slide fixture should match its public JSON format");

    ValidatedSources::new(transcript, slide_deck)
        .expect("the tiny course sources should satisfy every source invariant")
}
