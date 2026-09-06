use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fmt, io,
    path::Path,
    process::{Command, ExitStatus},
    string::FromUtf8Error,
};

use crate::{Slide, SlideDeck, SlideId};

const SPARSE_TEXT_THRESHOLD: usize = 8;
const MIN_REPEATED_FOOTER_PAGES: usize = 3;

pub struct ImportedDeck {
    pub slide_deck: SlideDeck,
    pub warnings: Vec<ImportWarning>,
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

/// Extracts one normalized slide per PDF page using Poppler's `pdftotext`.
pub fn import(path: &Path) -> Result<ImportedDeck, ImportError> {
    let output = Command::new("pdftotext")
        .args(["-raw", "-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output()
        .map_err(ImportError::CouldNotStartExtractor)?;
    if !output.status.success() {
        return Err(ImportError::ExtractionFailed {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let extracted = String::from_utf8(output.stdout).map_err(ImportError::InvalidUtf8)?;
    normalize_pages(&extracted)
}

fn normalize_pages(extracted: &str) -> Result<ImportedDeck, ImportError> {
    let mut pages: Vec<_> = extracted.split('\u{000c}').collect();
    if pages.last().is_some_and(|page| page.is_empty()) {
        pages.pop();
    }

    let mut page_lines: Vec<Vec<String>> = pages
        .iter()
        .map(|page| {
            page.lines()
                .map(|line| {
                    line.trim()
                        .chars()
                        .filter(|character| *character != '\u{2060}')
                        .collect::<String>()
                })
                .filter(|line| !line.is_empty())
                .collect()
        })
        .collect();
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

/// Checks extracted page text, including saved imports. `page` is the PDF's
/// one-based page number, used only to locate warnings in the original document.
pub fn page_text_warnings(page: u32, text: &str) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    let non_whitespace_characters = text.chars().filter(|c| !c.is_whitespace()).count();
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
    CouldNotStartExtractor(io::Error),
    ExtractionFailed { status: ExitStatus, stderr: String },
    InvalidUtf8(FromUtf8Error),
    TooManyPages,
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CouldNotStartExtractor(error) => {
                write!(formatter, "could not start pdftotext: {error}")
            }
            Self::ExtractionFailed { status, stderr } => {
                write!(formatter, "pdftotext failed with {status}: {stderr}")
            }
            Self::InvalidUtf8(_) => write!(formatter, "pdftotext returned invalid UTF-8"),
            Self::TooManyPages => write!(formatter, "PDF has more pages than can be assigned IDs"),
        }
    }
}

impl Error for ImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CouldNotStartExtractor(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
            Self::ExtractionFailed { .. } | Self::TooManyPages => None,
        }
    }
}
