# PDF text extraction backend

Date: 2026-08-29

## Recommendation

Keep invoking Poppler's `pdftotext` for the first PDF ingestion adapter. Do not
replace it with a Rust crate merely to make the implementation in-process.

This is an evidence-based, revisitable choice rather than a permanent backend
commitment. Poppler already extracts all 80 pages of the real Chinese Typst
deck with clean Chinese, symbols, and substantially better Rust/C++ token
spacing than the current pure-Rust candidates. PDF ingestion is an infrequent
boundary operation, so process startup is not on a meaningful hot path.

Keep the process dependency confined to `ingestion::pdf`. Reconsider it when
shipping a self-contained application, when a representative PDF corpus shows
Poppler failures, or when video alignment makes using one engine for both text
and page rendering materially valuable.

## Evidence from the real deck

The following comparison used Poppler 24.02.0, `pdf-extract` 0.12.0, and
`lopdf` 0.44.0 against `data/real_course/slides.pdf`:

| Backend | Pages returned | Observed text quality |
| --- | ---: | --- |
| `pdftotext -raw -enc UTF-8` | 80 | Chinese, bullets, symbols, and code were clean; code remained compact (`template <typename T>`, `x > y ? x : y`). |
| `pdf-extract` 0.12.0 | 80 | Chinese decoded, but layout heuristics inserted spaces within phrases and around code tokens (`Rust  语言`, `< typename  T >`, `x  >  y`). |
| `lopdf` 0.44.0 | 80 | Chinese decoded, but text-object boundaries produced pervasive line breaks and split code and headings into very small pieces. |

A local release-mode measurement also favored Poppler for this deck:
approximately 0.08 seconds and 16 MiB maximum resident memory, versus
approximately 0.35 seconds and 32 MiB for `pdf-extract`. These figures are not
a general benchmark; the fidelity difference is the decisive result.

Poppler's default mode and `-raw` were also compared. The default mode is meant
to reconstruct reading order, while the official manual calls raw content
stream order a hack and no longer recommends it generally. On this particular
slide deck, however, default mode split the multi-line footer and fragmented
code layout, whereas raw mode kept both compact. The current `-raw` choice is
therefore deck-tested, not a claim that raw mode is best for arbitrary PDFs.
[Poppler `pdftotext` manual](https://manpages.debian.org/unstable/poppler-utils/pdftotext.1.en.html)

## Candidate assessment

### Poppler `pdftotext`

Strengths:

- It has already passed the relevant Chinese deck, including CJK text,
  punctuation, bullets, mathematics, and code.
- It emits form-feed page boundaries, which directly supports one PDF page per
  slide.
- Poppler is a long-lived PDF engine with current releases and a dedicated
  encoding-data package. [Poppler project](https://poppler.freedesktop.org/)
- Calling it through `std::process::Command` keeps all PDF-specific behavior
  behind the ingestion boundary and avoids a C/C++ FFI surface.

Costs:

- `pdftotext` must be installed and discoverable on `PATH`; this complicates
  onboarding and especially Windows packaging.
- Results may vary with the system Poppler version unless deployments pin or
  report it.
- Poppler is GPL-licensed. The project currently invokes a separate executable
  rather than linking Poppler, but bundling or redistributing that executable
  still requires a deliberate license-compliance decision. Poppler's own
  README emphasizes that it is GPL rather than LGPL. This note is not legal
  advice. [Poppler README](https://gitlab.freedesktop.org/poppler/poppler/-/blob/master/README.md)

### `pdf-extract` 0.12.0

`pdf-extract` is the strongest current pure-Rust candidate. It is MIT-licensed,
has explicit per-page APIs, depends on `lopdf`, and adds its own font decoding,
Adobe CMap parsing, character positioning, and whitespace reconstruction. It
is therefore more than a thin call to `lopdf::Document::extract_text`.
[Crate API](https://docs.rs/pdf-extract/0.12.0/pdf_extract/)
[Manifest and dependencies](https://docs.rs/crate/pdf-extract/0.12.0/source/Cargo.toml)

It is not the better choice today:

- Its output was materially worse for retrieval-relevant tokens in the actual
  deck.
- Its per-page implementation increments page numbers until extraction returns
  an error, then returns the pages accumulated so far. A page-specific failure
  can therefore look like a successfully shortened document unless the caller
  separately verifies the page count.
  [Versioned source](https://docs.rs/crate/pdf-extract/0.12.0/source/src/lib.rs)
- Current font/CMap paths still contain `todo!`, `panic!`, and `unwrap` branches
  for unsupported structures, which is unattractive for user-supplied PDFs.
  [Versioned source](https://docs.rs/crate/pdf-extract/0.12.0/source/src/lib.rs)

The crate is active enough to evaluate again, but its present result does not
justify exchanging proven fidelity for simpler installation.

### `lopdf` 0.44.0 directly

`lopdf` is active, MIT-licensed, exposes page lookup and text extraction, and
has supported ToUnicode CMaps since 0.34.0. Its newer bounded extraction APIs
are useful when handling untrusted compressed streams.
[Current API](https://docs.rs/lopdf/0.44.0/lopdf/struct.Document.html)
[Changelog](https://github.com/J-F-Liu/lopdf/blob/main/CHANGELOG.md)
[License](https://github.com/J-F-Liu/lopdf/blob/main/LICENSE)

It is a PDF object/parser library first, not a mature reading-order engine. Its
extractor primarily decodes text-showing operations and approximates text
object endings as newlines. The real-deck output confirmed that this is too
low-level for BeyondSlides without implementing substantial layout logic. That
would make PDF extraction our maintenance problem and duplicate work already
done more successfully by Poppler.

### `pdfium-render`

`pdfium-render` provides a well-documented per-page Unicode text API over
Chromium's PDFium, but the Rust crate does not contain PDFium. A deployment must
still supply, bundle, or statically link a platform-specific native PDFium
library. It changes the native dependency rather than eliminating one.
[Binding and packaging documentation](https://docs.rs/pdfium-render/latest/pdfium_render/)
[Per-page text API](https://docs.rs/pdfium-render/latest/pdfium_render/prelude/struct.PdfPageText.html)

The crate is MIT/Apache-2.0 and PDFium uses permissive redistribution terms,
but PDFium's larger native packaging surface is unnecessary for text-only
ingestion. It becomes worth evaluating if BeyondSlides later wants one engine
for page rendering, visual alignment, and text extraction.
[Crate license](https://github.com/ajrcarey/pdfium-render/blob/master/LICENSE.md)
[PDFium license](https://pdfium.googlesource.com/pdfium/+/refs/heads/main/LICENSE)

## Follow-up guardrails

1. Keep normalization and footer removal independent of the extraction
   process so another backend can reuse them.
2. Report the detected `pdftotext` version and an actionable installation
   error in the eventual end-to-end CLI.
3. Preserve a small Chinese/code PDF regression fixture and assert page count
   and representative tokens.
4. Before distribution, choose a repository license and review the obligations
   for installing versus bundling Poppler.
5. Re-run candidates against a labeled, representative PDF corpus rather than
   switching on architectural aesthetics alone.
