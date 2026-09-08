# Rain Classroom slide import

Research checked against Rain Classroom's authenticated first-party web APIs
and frontend bundle on 2026-09-08. This is an implementation probe, not a
stable provider contract: the endpoints are private and undocumented. The
probe used an authorized saved session. No cookie, token, signed URL, user
identity, or course title was recorded.

## Verified acquisition chain

The student web application exposes enough information to recover a lecture's
rendered slide pages:

1. `GET /v2/api/web/logs/learn/{classroom_id}` returns
   `data.activities[]`. Lecture activities have `type`, `courseware_id`, and
   `title`; for the tested account, `type == 14` identifies classroom lectures.
2. `GET /api/v3/classroom-report/replay?lesson_id=...` returns
   `data.presentations[]` as presentation-ID strings alongside the replay
   metadata. A lecture can contain more than one presentation: one of the seven
   tested lectures contained two.
3. `GET /api/v3/lesson-summary/student/presentation?lesson_id=...&presentation_id=...`
   returns `data.presentation` (`id`, `title`, `width`, `height`, `cover`,
   `conf`, and counters) and `data.slides[]`. Each slide has an `index`, `id`,
   `cover`, `coverProvider`, and interaction fields.

These endpoint families also appear in Rain Classroom's
[first-party frontend bundle](https://fe-static-yuketang.yuketang.cn/fe/static/web/1.2.299/js/pc.55efc450.js),
loaded by the [Rain Classroom web app](https://pro.yuketang.cn/v2/web/index).
The response shapes above were verified from the authenticated JSON responses,
not inferred only from string constants in the bundle.

The tested seven-lecture course exposed eight presentations and 519 slide
pages. Every presentation and slide asset URL in those responses pointed to a
private Rain Classroom Qiniu host. A fetched slide returned `image/jpeg`; its
URL had no filename extension and carried `e` and `token` query parameters.

## No verified original PDF

The verified lecture/replay chain exposes rendered page images, not the
original uploaded PPT/PPTX/PDF. Across all eight tested presentations:

- neither presentation nor slide responses contained a PDF, PPT, original-file,
  or download URL field;
- the only asset URLs were presentation-cover and slide-cover images; and
- the frontend bundle's `/v/cards/ppt_download_url` constant did not establish
  an applicable student-playback flow. Read-only probes using the presentation
  ID returned `success: false` for every tested presentation.

This does **not** prove that no teacher account, deck-library screen, or future
Rain Classroom version can download an original file. It means BeyondSlides
must not promise an original PDF through the student lecture interface we have
verified.

## Implementation consequences

For the first version, a Rain Classroom written source should be described as
"import slides," not "download the original PDF." The safe materialization is:

1. let the user select a course, lecture, and—when there are several—a specific
   presentation;
2. fetch its slide covers promptly and order them by `slide.index`; and
3. assemble those JPEG pages into the canonical `slides.pdf` expected by the
   rest of BeyondSlides.

That PDF is raster-only. It preserves the visible slides and works for page
rendering, but it has no native text layer, selectable text, speaker notes,
animations, or original vector/code structure. Consequently, the existing
`pdftotext` ingestion path will usually produce little or no useful text for a
Rain Classroom-imported deck. Keeping the same downstream PDF seam is still
reasonable for the current restructuring, but OCR or image-aware extraction is
required for meaningful written-source retrieval. That later implementation is
now recorded by ADR 0010: native PP-OCRv5 supplies fallback text for sparse
Rain Classroom pages while the raster PDF remains canonical.

Slide-cover URLs are short-lived bearer-like capabilities and may expose course
material to anyone who receives them before expiry. BeyondSlides should keep
them server-side, never persist or log them, validate HTTPS, download them
immediately, and persist only the resulting local `slides.pdf`. Authentication
cookies require the same treatment. A failed or expired asset request should be
resolved by refreshing presentation metadata through the authenticated API,
not by retrying a stored signed URL indefinitely.
