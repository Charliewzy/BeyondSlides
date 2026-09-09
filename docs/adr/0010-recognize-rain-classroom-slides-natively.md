# Recognize Rain Classroom slides with native PP-OCRv5

PDF extraction and rendering in this decision are superseded by ADR 0011; the
native PP-OCRv5 fallback remains accepted.

Rain Classroom exposes courseware as page images, while the canonical written
source remains a PDF and ordinary PDFs often contain better embedded text. Keep
using embedded text, but use CPU PP-OCRv5 text only for pages below the sparse
text threshold. The application downloads pinned ONNX detector, recognizer, and
dictionary assets into its shared model cache, verifies their SHA-256 digests,
and runs them in-process through the existing ONNX Runtime; it does not require
Python, PaddlePaddle, an OCR service, or a GPU. OCR does not change page order or
slide IDs, and pages that remain sparse continue to be surfaced for review.
