# Use page-level OCR for all slide sources

Status: accepted.

Rain Classroom and uploaded PDFs both become the same canonical slide PDF, so
the import source should not decide whether a page receives OCR. PDFium first
extracts embedded text. Pages with sparse or broken text, or substantial image
objects, are rendered and recognized locally with PP-OCRv5; plain searchable
pages skip OCR and do not require its model download.

For each page, keep PDFium text when it is usable. Replace sparse text with a
substantial OCR result; on mixed pages, append OCR lines that are not already
represented by the embedded text. Unicode normalization and approximate
matching avoid doubling lines with minor recognition differences. Preserve the
original PDF and page order for display. OCR is a best-effort extraction aid,
so sparse or suspicious pages remain visible in import review.
