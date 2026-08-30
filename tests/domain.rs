use beyond_slides::{Slide, SlideDeck, SlideId};

#[test]
fn slide_deck_finds_a_slide_by_its_canonical_id() {
    let slide_deck = SlideDeck {
        slides: vec![
            Slide {
                id: SlideId(0),
                text: "First in presentation order".to_owned(),
            },
            Slide {
                id: SlideId(1),
                text: "Second in presentation order".to_owned(),
            },
        ],
    };

    let found = slide_deck.find(SlideId(1));

    assert_eq!(
        found.map(|slide| slide.text.as_str()),
        Some("Second in presentation order")
    );
}
