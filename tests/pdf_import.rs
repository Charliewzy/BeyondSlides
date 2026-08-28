use std::{error::Error, path::Path};

use beyond_slides::ingestion::pdf::{ImportWarning, import};
use beyond_slides::{Slide, SlideId};

#[test]
fn pdf_pages_become_slides_without_a_repeated_footer() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_import.pdf");

    let imported = import(&fixture)?;

    assert_eq!(
        imported.slide_deck.slides,
        vec![
            Slide {
                id: SlideId(1),
                text: "First slide".to_owned(),
            },
            Slide {
                id: SlideId(2),
                text: "Second slide".to_owned(),
            },
            Slide {
                id: SlideId(3),
                text: String::new(),
            },
            Slide {
                id: SlideId(4),
                text: "Last slide".to_owned(),
            },
        ]
    );
    Ok(())
}

#[test]
fn pdf_import_reports_sparse_pages_without_dropping_them() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_import.pdf");

    let imported = import(&fixture)?;

    assert_eq!(
        imported.warnings,
        vec![ImportWarning::SparseText {
            page: 3,
            non_whitespace_characters: 0,
        }]
    );
    Ok(())
}

#[test]
fn pdf_import_aggregates_suspicious_glyphs_per_page() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_warning.pdf");

    let imported = import(&fixture)?;

    assert_eq!(
        imported.warnings,
        vec![ImportWarning::SuspiciousGlyphs {
            page: 1,
            glyphs: vec!['□', '�'],
        }]
    );
    Ok(())
}

#[test]
fn pdf_import_removes_invisible_word_joiners() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_warning.pdf");

    let imported = import(&fixture)?;

    assert!(!imported.slide_deck.slides[0].text.contains('\u{2060}'));
    Ok(())
}
