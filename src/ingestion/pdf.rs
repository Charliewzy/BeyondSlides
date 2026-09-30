use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt,
    path::Path,
};

use image::DynamicImage;
use once_cell::sync::OnceCell;
use pdfium_render::prelude::{
    PdfPageObjectCommon, PdfPageObjectsCommon, PdfPageRenderRotation, PdfRenderConfig, Pdfium,
    PdfiumError,
};
use similar::{Algorithm, DiffTag, capture_diff_slices};
use unicode_normalization::UnicodeNormalization;

use crate::{Slide, SlideDeck, SlideId, runtime_tools};

const SPARSE_TEXT_THRESHOLD: usize = 8;
const MIN_REPEATED_FOOTER_PAGES: usize = 3;

static PDFIUM: OnceCell<Pdfium> = OnceCell::new();

pub struct ImportedDeck {
    pub slide_deck: SlideDeck,
    pub warnings: Vec<ImportWarning>,
}

/// Pages worth rendering for OCR in a manually uploaded PDF.
pub struct OcrPlan {
    pub page_count: usize,
    pub page_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportWarning {
    SparseText {
        page: u32,
        non_whitespace_characters: usize,
    },
    SuspiciousGlyphs {
        page: u32,
        glyphs: Vec<char>,
    },
}

/// Extracts one normalized slide per PDF page using managed PDFium.
pub fn import(path: &Path) -> Result<ImportedDeck, ImportError> {
    import_with_page_text(path, None)
}

/// Extracts PDF text first, then fills sparse pages or adds OCR-only lines.
pub fn import_with_ocr(path: &Path, ocr_pages: &[String]) -> Result<ImportedDeck, ImportError> {
    import_with_page_text(path, Some(ocr_pages))
}

/// Plain searchable pages do not need OCR. Sparse or broken text and sizable
/// embedded images may contain words that PDFium cannot extract.
pub fn plan_ocr(path: &Path) -> Result<OcrPlan, ImportError> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(ImportError::Pdfium)?;
    let page_count = document.pages().len() as usize;
    let mut page_indices = Vec::new();
    for (index, page) in document.pages().iter().enumerate() {
        let text = page
            .text()
            .map(|text| text.all())
            .map_err(ImportError::Pdfium)?;
        let sparse_or_broken = (non_whitespace_characters(&text) < SPARSE_TEXT_THRESHOLD
            && !page.objects().is_empty())
            || text.contains(['\u{25a1}', '\u{fffd}']);
        let page_area = page.width().value * page.height().value;
        let substantial_image =
            page.objects().iter().any(|object| {
                object.as_image_object().is_some()
                    && object.width().ok().zip(object.height().ok()).is_some_and(
                        |(width, height)| width.value * height.value >= page_area * 0.05,
                    )
            });
        if sparse_or_broken || substantial_image {
            page_indices.push(index);
        }
    }
    Ok(OcrPlan {
        page_count,
        page_indices,
    })
}

fn import_with_page_text(
    path: &Path,
    ocr_pages: Option<&[String]>,
) -> Result<ImportedDeck, ImportError> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(ImportError::Pdfium)?;
    let pages = document
        .pages()
        .iter()
        .map(|page| page.text().map(|text| text.all()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ImportError::Pdfium)?;
    normalize_pages(&pages, ocr_pages)
}

fn normalize_pages(
    pages: &[String],
    ocr_pages: Option<&[String]>,
) -> Result<ImportedDeck, ImportError> {
    if let Some(ocr_pages) = ocr_pages
        && ocr_pages.len() != pages.len()
    {
        return Err(ImportError::OcrPageCountMismatch {
            pdf_pages: pages.len(),
            ocr_pages: ocr_pages.len(),
        });
    }
    let mut page_lines: Vec<Vec<String>> =
        pages.iter().map(|page| normalized_lines(page)).collect();
    if let Some(ocr_pages) = ocr_pages {
        for (lines, ocr_text) in page_lines.iter_mut().zip(ocr_pages) {
            let extracted_characters = non_whitespace_characters(&lines.join("\n"));
            let ocr_lines = normalized_lines(ocr_text);
            let ocr_characters = non_whitespace_characters(&ocr_lines.join("\n"));
            if extracted_characters < SPARSE_TEXT_THRESHOLD
                && ocr_characters >= SPARSE_TEXT_THRESHOLD
            {
                *lines = ocr_lines;
            } else {
                for ocr_line in ocr_lines {
                    let candidate = comparable_text(&ocr_line);
                    if candidate.chars().count() < 4 || ocr_line_is_duplicate(&candidate, lines) {
                        continue;
                    }
                    lines.push(ocr_line);
                }
            }
        }
    }
    let repeated_footer = repeated_footer(&page_lines);

    let mut slides = Vec::with_capacity(page_lines.len());
    let mut warnings = Vec::new();
    for (position, lines) in page_lines.iter_mut().enumerate() {
        if let (Some(repeated_footer), Some(last_line)) = (&repeated_footer, lines.last())
            && canonical_footer(last_line) == *repeated_footer
        {
            lines.pop();
        }

        let id = u32::try_from(position)
            .map(SlideId)
            .map_err(|_| ImportError::TooManyPages)?;
        let page = id.0.checked_add(1).ok_or(ImportError::TooManyPages)?;
        let text = lines.join("\n");
        warnings.extend(page_text_warnings(page, &text));
        slides.push(Slide { id, text });
    }

    Ok(ImportedDeck {
        slide_deck: SlideDeck { slides },
        warnings,
    })
}

/// Renders every PDF page at a fixed target width while preserving aspect ratio.
pub fn render_pages_to_width(path: &Path, width: u32) -> Result<Vec<DynamicImage>, ImportError> {
    render_pages(path, PageSelection::All, RenderSize::Width(width))
}

/// Renders one zero-based PDF page within a square bounding box.
pub fn render_page_to_fit(
    path: &Path,
    page_index: usize,
    maximum_dimension: u32,
) -> Result<DynamicImage, ImportError> {
    render_pages(
        path,
        PageSelection::One(page_index),
        RenderSize::Fit(maximum_dimension),
    )?
    .pop()
    .ok_or(ImportError::MissingPage { page_index })
}

/// Renders every PDF page into an exact pixel size.
pub fn render_pages_exact(
    path: &Path,
    width: u32,
    height: u32,
) -> Result<Vec<DynamicImage>, ImportError> {
    render_pages(path, PageSelection::All, RenderSize::Exact(width, height))
}

#[derive(Clone, Copy)]
enum PageSelection {
    All,
    One(usize),
}

#[derive(Clone, Copy)]
enum RenderSize {
    Width(u32),
    Fit(u32),
    Exact(u32, u32),
}

fn render_pages(
    path: &Path,
    selection: PageSelection,
    size: RenderSize,
) -> Result<Vec<DynamicImage>, ImportError> {
    let pdfium = bind_pdfium()?;
    let document = pdfium
        .load_pdf_from_file(path, None)
        .map_err(ImportError::Pdfium)?;
    let indices: Vec<_> = match selection {
        PageSelection::All => (0..document.pages().len() as usize).collect(),
        PageSelection::One(index) => vec![index],
    };
    let mut rendered = Vec::with_capacity(indices.len());
    for page_index in indices {
        let page = document
            .pages()
            .get(i32::try_from(page_index).map_err(|_| ImportError::MissingPage { page_index })?)
            .map_err(|_| ImportError::MissingPage { page_index })?;
        let config = match size {
            RenderSize::Width(width) => PdfRenderConfig::new().set_target_width(width as i32),
            RenderSize::Fit(maximum) => PdfRenderConfig::new()
                .set_maximum_width(maximum as i32)
                .set_maximum_height(maximum as i32),
            RenderSize::Exact(width, height) => {
                PdfRenderConfig::new().set_fixed_size(width as i32, height as i32)
            }
        }
        .rotate_if_landscape(PdfPageRenderRotation::None, false);
        rendered.push(
            page.render_with_config(&config)
                .and_then(|bitmap| bitmap.as_image())
                .map_err(ImportError::Pdfium)?,
        );
    }
    Ok(rendered)
}

fn bind_pdfium() -> Result<&'static Pdfium, ImportError> {
    PDFIUM.get_or_try_init(|| {
        let library = runtime_tools::pdfium_library_path().map_err(ImportError::RuntimeTool)?;
        Pdfium::bind_to_library(library)
            .map(Pdfium::new)
            .map_err(ImportError::Pdfium)
    })
}

fn normalized_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.trim()
                .chars()
                .filter(|character| *character != '\u{2060}')
                .collect::<String>()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn non_whitespace_characters(text: &str) -> usize {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .count()
}

/// Ignore layout and ordinary punctuation, while retaining programming
/// operators so that `C++` and `C` do not become the same evidence.
fn comparable_text(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '+' | '-' | '*' | '/' | '=' | '<' | '>' | '&' | '|' | '#' | '_' | '%'
                )
        })
        .collect()
}

fn ocr_line_is_duplicate(candidate: &str, existing_lines: &[String]) -> bool {
    let existing = comparable_text(&existing_lines.join(""));
    if existing.contains(candidate) {
        return true;
    }
    let candidate: Vec<char> = candidate.chars().collect();
    if candidate.len() < 6 {
        return false;
    }
    let existing: Vec<char> = existing.chars().collect();
    let operations = capture_diff_slices(Algorithm::Myers, &existing, &candidate);
    let mut matched = 0;
    let mut first = None;
    let mut last = 0;
    for operation in operations {
        if operation.tag() == DiffTag::Equal {
            let range = operation.old_range();
            matched += range.len();
            first.get_or_insert(range.start);
            last = range.end;
        }
    }
    let nearby = first.is_some_and(|first| last - first <= candidate.len() + candidate.len() / 5);
    nearby && matched * 100 >= candidate.len() * 85
}

/// Checks extracted page text, including saved imports. `page` is the PDF's
/// one-based page number, used only to locate warnings in the original document.
pub fn page_text_warnings(page: u32, text: &str) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    let non_whitespace_characters = non_whitespace_characters(text);
    if non_whitespace_characters < SPARSE_TEXT_THRESHOLD {
        warnings.push(ImportWarning::SparseText {
            page,
            non_whitespace_characters,
        });
    }
    let glyphs: Vec<_> = text
        .chars()
        .filter(|c| matches!(c, '\u{25a1}' | '\u{fffd}'))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !glyphs.is_empty() {
        warnings.push(ImportWarning::SuspiciousGlyphs { page, glyphs });
    }
    warnings
}

fn repeated_footer(pages: &[Vec<String>]) -> Option<String> {
    let mut frequencies = HashMap::new();
    for footer in pages.iter().filter_map(|page| page.last()) {
        *frequencies.entry(canonical_footer(footer)).or_insert(0) += 1;
    }

    frequencies
        .into_iter()
        .filter(|(_, count)| *count >= MIN_REPEATED_FOOTER_PAGES && count * 2 > pages.len())
        .max_by_key(|(_, count)| *count)
        .map(|(footer, _)| footer)
}

fn canonical_footer(line: &str) -> String {
    let mut words: Vec<_> = line.split_whitespace().collect();
    if words.len() >= 3 {
        let page_number = &words[words.len() - 3..];
        if page_number[0].bytes().all(|byte| byte.is_ascii_digit())
            && page_number[1] == "/"
            && page_number[2].bytes().all(|byte| byte.is_ascii_digit())
        {
            words.truncate(words.len() - 3);
        }
    }
    words.join(" ")
}

#[derive(Debug)]
pub enum ImportError {
    RuntimeTool(runtime_tools::RuntimeToolError),
    Pdfium(PdfiumError),
    OcrPageCountMismatch { pdf_pages: usize, ocr_pages: usize },
    MissingPage { page_index: usize },
    TooManyPages,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeTool(error) => write!(formatter, "could not prepare PDFium: {error}"),
            Self::Pdfium(error) => write!(formatter, "could not process PDF with PDFium: {error}"),
            Self::OcrPageCountMismatch {
                pdf_pages,
                ocr_pages,
            } => write!(
                formatter,
                "PDF contains {pdf_pages} pages but OCR returned {ocr_pages} pages"
            ),
            Self::MissingPage { page_index } => {
                write!(
                    formatter,
                    "PDF does not contain zero-based page {page_index}"
                )
            }
            Self::TooManyPages => write!(formatter, "PDF has more pages than can be assigned IDs"),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RuntimeTool(error) => Some(error),
            Self::Pdfium(error) => Some(error),
            Self::OcrPageCountMismatch { .. } | Self::MissingPage { .. } | Self::TooManyPages => {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_replaces_only_sparse_pages_before_footer_removal() {
        let imported = normalize_pages(
            &[
                String::new(),
                "native searchable text\nfooter 2 / 3".into(),
                "native third page\nfooter 3 / 3".into(),
            ],
            Some(&[
                "OCR title\nfooter 1 / 3".into(),
                "native searchable text\nfooter 2 / 3".into(),
                "native third page\nfooter 3 / 3".into(),
            ]),
        )
        .unwrap();

        assert_eq!(imported.slide_deck.slides[0].text, "OCR title");
        assert_eq!(imported.slide_deck.slides[1].text, "native searchable text");
        assert_eq!(imported.slide_deck.slides[2].text, "native third page");
        assert!(imported.warnings.is_empty());
    }

    #[test]
    fn ocr_adds_text_missing_from_an_otherwise_searchable_page() {
        let imported = normalize_pages(
            &["可检索标题\nC++ 模板函数".into()],
            Some(&["可检索标题\nC++ 模板函数\n截图中的编译错误\n截图中的编译错误".into()]),
        )
        .unwrap();

        assert_eq!(
            imported.slide_deck.slides[0].text,
            "可检索标题\nC++ 模板函数\n截图中的编译错误"
        );
    }

    #[test]
    fn ocr_errors_do_not_duplicate_pdf_text() {
        let imported = normalize_pages(
            &["机器学习模型的训练过程".into()],
            Some(&["机器学刁模型的训练过程\n额外的实践建议".into()]),
        )
        .unwrap();

        assert_eq!(
            imported.slide_deck.slides[0].text,
            "机器学习模型的训练过程\n额外的实践建议"
        );
    }

    #[test]
    fn ocr_page_count_must_match_the_pdf() {
        assert!(matches!(
            normalize_pages(&["one".into(), "two".into()], Some(&["one".into()])),
            Err(ImportError::OcrPageCountMismatch {
                pdf_pages: 2,
                ocr_pages: 1
            })
        ));
    }
}
