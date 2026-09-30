use std::{error::Error, path::Path};

use beyond_slides::ingestion::pdf::{ImportWarning, import, plan_ocr};
use beyond_slides::{Slide, SlideId};

#[test]
fn saved_page_text_uses_the_same_checks_and_can_have_multiple_warnings() {
    use beyond_slides::ingestion::pdf::page_text_warnings;
    assert_eq!(
        page_text_warnings(7, " □ □ � "),
        vec![
            ImportWarning::SparseText {
                page: 7,
                non_whitespace_characters: 3
            },
            ImportWarning::SuspiciousGlyphs {
                page: 7,
                glyphs: vec!['□', '�']
            },
        ]
    );
    assert!(page_text_warnings(1, "正常文字提取共八字").is_empty());
}

#[test]
fn pdf_pages_become_slides_without_a_repeated_footer() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_import.pdf");

    let imported = import(&fixture)?;

    assert_eq!(
        imported.slide_deck.slides,
        vec![
            Slide {
                id: SlideId(0),
                text: "First slide".to_owned(),
            },
            Slide {
                id: SlideId(1),
                text: "Second slide".to_owned(),
            },
            Slide {
                id: SlideId(2),
                text: String::new(),
            },
            Slide {
                id: SlideId(3),
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
fn ocr_plan_skips_searchable_and_blank_pages_but_flags_broken_text() -> Result<(), Box<dyn Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let ordinary = plan_ocr(&fixtures.join("pdf_import.pdf"))?;
    assert_eq!(ordinary.page_count, 4);
    assert!(ordinary.page_indices.is_empty());

    let broken = plan_ocr(&fixtures.join("pdf_warning.pdf"))?;
    assert_eq!(broken.page_count, 1);
    assert_eq!(broken.page_indices, vec![0]);
    Ok(())
}

#[test]
fn pdf_import_removes_invisible_word_joiners() -> Result<(), Box<dyn Error>> {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pdf_warning.pdf");

    let imported = import(&fixture)?;

    assert!(!imported.slide_deck.slides[0].text.contains('\u{2060}'));
    Ok(())
}
