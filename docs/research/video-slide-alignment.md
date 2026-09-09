# Video-to-slide alignment

> Historical research: the current implementation uses managed FFmpeg and
> PDFium rather than system Poppler; see
> [ADR 0011](../adr/0011-manage-native-media-and-pdf-runtimes.md).

Date: 2026-08-29

## Recommendation

Use **visual matching between timestamped video frames and rendered PDF pages**
as the primary source of slide-to-time alignment. The recording already contains
the strongest available evidence of which slide was visible; transcript
retrieval answers a different, semantic question and should not be asked to
manufacture exact presentation times.

The first implementation should remain CPU-first:

1. Render every PDF page to a consistently sized reference image.
2. Sample one timestamped video frame per second and locate the slide region.
3. Compare that region with the page images using a cheap shortlist followed by
   a stronger registered-image score.
4. Decode the sequence with a temporal model that strongly prefers remaining on
   the same page or moving to an adjacent page, but can represent backward
   navigation, jumps, and an unknown state.
5. Re-scan only the one-second neighborhood around each inferred transition at a
   higher frame rate to obtain a finer boundary.

At one frame per second, the two-hour, 80-page real course produces about
576,000 coarse frame-page comparisons. That is a modest search space after the
images are downscaled and the cheap stage shortlists candidates. Expensive
feature matching should not be run indiscriminately on every full-resolution
pair.

The result should be a sequence of page intervals with confidence and supporting
frame timestamps. Low-confidence intervals should remain unknown rather than be
silently filled with a plausible page.

## Proposed pipeline

### 1. Render stable page references

Render each PDF page to PNG at a fixed resolution and preserve the PDF page
number as its explicit slide ID. Poppler's `pdftoppm` writes one image per page,
accepts a requested DPI, and can emit PNG.
[Poppler `pdftoppm` manual](https://manpages.debian.org/bookworm/poppler-utils/pdftoppm.1.en.html)

The current project already depends on Poppler for PDF ingestion, so this adds no
new platform dependency to the prototype. Reference images should be normalized
to the same aspect ratio and grayscale size used for coarse matching, while the
original render is retained for feature registration.

### 2. Extract frames with real timestamps

Use FFmpeg to obtain a coarse one-frame-per-second stream. Its `fps` filter
converts a video to a requested constant frame rate by dropping or duplicating
frames, and its `select` filter can instead choose frames according to their
presentation timestamp. FFmpeg documents both selecting frames at a minimum
time separation and selecting likely scene changes.
[FFmpeg filter documentation](https://ffmpeg.org/ffmpeg-filters.html#fps)
[FFmpeg `select` documentation](https://ffmpeg.org/ffmpeg-filters.html#select_002c-aselect)

Do not infer time from a decoded frame's collection index. Carry its presentation
timestamp through the alignment data. FFmpeg's image muxer can put a frame PTS
in its filename, and `ffprobe -show_frames` emits machine-readable per-frame
records when exact frame metadata is needed.
[FFmpeg image2 documentation](https://ffmpeg.org/ffmpeg-formats.html#image2-1)
[ffprobe documentation](https://ffmpeg.org/ffprobe.html)

Scene-change detection is useful for proposing additional samples and narrowing
transition neighborhoods, but it is not sufficient by itself: a camera cut,
lecturer motion, animation, or pointer movement can also change the frame. FFmpeg
exposes both a scene score and scene-change timestamps, so this signal can remain
one piece of evidence rather than become a page label.
[FFmpeg `scdet` documentation](https://ffmpeg.org/ffmpeg-filters.html#scdet)

### 3. Locate and register the slide region

For a fixed screen-recording layout, a configured crop is the simplest and most
reliable option. If the projected slide is photographed at an angle or moves in
the frame, match local features between a PDF page and the video frame, estimate
a homography from the matches, and rectify the slide region before comparing
pixels. OpenCV's official tutorials demonstrate this exact known-planar-object
workflow using feature descriptors, matching, `findHomography`, and
`perspectiveTransform`; they also demonstrate CPU-oriented ORB and AKAZE
tracking with RANSAC inlier filtering.
[OpenCV feature matching and homography tutorial](https://docs.opencv.org/4.x/d1/de0/tutorial_py_feature_homography.html)
[OpenCV AKAZE and ORB tracking tutorial](https://docs.opencv.org/4.11.0/dc/d16/tutorial_akaze_tracking.html)

This is established specifically for lecture material. Wang, Subramanian, and
Kankanhalli align electronic slides with video under defocus, speaker occlusion,
and camera pan, tilt, and zoom using visual cues plus a temporal decision model.
[Wang et al., *A Robust Framework for Aligning Lecture Slides with Video*](https://www.comp.nus.edu.sg/~mohan/papers/icip2009.pdf)
Fan et al. likewise treat a slide as a planar image and estimate homographies
between slide images and presentation-video frames, jointly refining them over a
frame sequence.
[IBM Research publication](https://research.ibm.com/publications/accurate-alignment-of-presentation-slides-with-educational-video)

### 4. Score visual identity in two stages

Use a perceptual image hash or a very small grayscale image comparison to
shortlist pages. OpenCV's image-hash module exists specifically to extract image
hashes and find similar images efficiently; it includes pHash, block-mean hash,
and several other algorithms.
[OpenCV image-hash documentation](https://docs.opencv.org/4.12.0/d4/d93/group__img__hash.html)

These classical comparisons have different roles rather than being
interchangeable:

- a perceptual hash is cheap enough to compare with every page, but its compact
  representation can confuse near-duplicate builds and should only shortlist;
- template matching slides one rectangular image patch across another image, so
  it is useful for a fixed layout with translation but does not by itself solve
  perspective or substantial scale changes;
- SSIM is a full-reference structural comparison, so it is useful after the two
  images have been registered to the same geometry;
- local-feature matching plus a RANSAC homography is the more expensive path
  that handles a planar slide photographed at an angle and supplies geometric
  inliers as independently inspectable evidence.

[OpenCV template-matching tutorial](https://docs.opencv.org/4.x/de/da9/tutorial_template_matching.html)
[Original SSIM paper and implementation](https://www.cns.nyu.edu/~lcv/pubs/makeAbs.php?loc=Wang03)

For shortlisted pages:

- if the slide crop is already stable, resize both images identically and use a
  structural similarity score;
- if the crop has perspective or scale changes, first score the number and
  spatial consistency of feature inliers, rectify with the estimated homography,
  and then compute structural similarity;
- optionally compare edge images as another cue when projector color and
  exposure differ from the PDF render.

OpenCV's reference SSIM implementation compares two images and returns a
per-channel score from 0 (worst) to 1 (best).
[OpenCV `QualitySSIM`](https://docs.opencv.org/4.9.0/d9/db5/classcv_1_1quality_1_1QualitySSIM.html)

No single visual score should be treated as a calibrated probability. The
useful evidence includes the winning score, its margin over the runner-up, the
number of geometrically consistent feature matches, and agreement with nearby
frames.

#### Simplest prototype for a direct screen recording

Do not start with OpenCV. For a recording whose slide rectangle stays fixed,
the first useful experiment can remain inside the Rust process after FFmpeg has
extracted frames:

1. Configure one crop rectangle, resize the crop and every rendered page to the
   same small grayscale dimensions, and compute each page hash once.
2. Use `imageproc`'s 64-bit DCT perceptual hash to compare every sampled frame
   with every page by Hamming distance and retain a small candidate set. Its
   `fft` feature enables this implementation. Pin the crate version if hashes
   are persisted: its API explicitly warns that hash values may vary between
   versions.
   [`imageproc` pHash API](https://docs.rs/imageproc/0.27.0/imageproc/image_hash/struct.PHash.html)
   [`imageproc` crate features](https://docs.rs/imageproc/0.27.0/imageproc/#crate-features)
3. Re-rank those candidates with `image_compare`'s grayscale MSSIM and retain
   the score and difference map for inspection. The crate's structural
   comparison requires equal-sized images and returns both an aggregate score
   and per-pixel similarity data.
   [`image_compare` documentation](https://docs.rs/image-compare/0.5.0/image_compare/)

Both crates build on Rust's `image` ecosystem; `image_compare` declares only
Rust library dependencies for its normal build, so this path avoids adding a
native OpenCV runtime merely to test stable, registered frames.
[`image_compare` package manifest](https://docs.rs/crate/image-compare/0.5.0/source/Cargo.toml)

The Rust OpenCV bindings, by contrast, require a system OpenCV installation and
Clang and describe their own API as unstable and not yet extensively tested.
[`opencv` crate package documentation](https://docs.rs/crate/opencv/0.100.1)

A fixed-overlay mask is **not required up front**. First crop away borders,
player chrome, and picture-in-picture regions that lie outside the slide. If a
fixed subtitle bar, watermark, or camera inset overlaps the slide and repeatedly
changes the winner, exclude the same pixels from both the video crop and every
page reference before hashing and MSSIM. This is an inference from the matching
algorithms: OpenCV likewise makes masks optional for template matching, and
documents that only `TM_SQDIFF` and `TM_CCORR_NORMED` accept them.
[OpenCV template-matching masks](https://docs.opencv.org/4.x/de/da9/tutorial_template_matching.html#autotoc_md324)

A limited spot check supports leaving masks out initially. Four frames from the
real recording at 120, 1,800, 4,500, and 7,500 seconds showed a stable full-slide
capture plus a translucent upper-right overlay and lower-center sharing strip.
Unmasked full-frame FFmpeg SSIM selected the correct page from all 80 pages at
each timestamp, with margins of 0.092 to 0.200 over the runner-up. In a separate
four-frame comparison against only the two adjacent pages, masking both overlay
rectangles changed the margins from 0.060, 0.111, 0.061, and 0.130 to 0.057,
0.109, 0.061, and 0.130: it raised absolute scores slightly but did not improve
this limited ranking. These small samples justify the simplest prototype, not a
general claim that masks never help.

Escalate only when a labeled sample demonstrates a specific failure:

- use `imageproc` template matching when the slide is merely translated within
  a stable frame; it provides ordinary, masked, and parallel sliding-window
  variants, but does not address scale or perspective;
- use ORB matches plus RANSAC homography when scale, perspective, or camera
  movement prevents a single crop from registering the images; OpenCV describes
  ORB as a lower-compute feature detector/descriptor and uses Hamming distance
  for its default binary descriptors;
- consider a learned feature only if classical registration still fails under
  blur, severe appearance changes, or persistent occlusion.

[`imageproc` template matching](https://docs.rs/imageproc/0.27.0/imageproc/template_matching/)
[OpenCV ORB tutorial](https://docs.opencv.org/4.x/d1/d89/tutorial_py_orb.html)

FFmpeg's similarly named filters do not replace this page-classification loop.
Its `ssim` filter compares two synchronized video streams frame by frame and
requires equal resolution, pixel format, and frame count; its `signature`
filter computes MPEG-7 signatures to decide whether whole videos or subsequences
match. They are useful command-line diagnostics, but awkward for ranking one
video frame against 80 independent page images.
[FFmpeg `ssim` filter](https://ffmpeg.org/ffmpeg-filters.html#ssim)
[FFmpeg `signature` filter](https://ffmpeg.org/ffmpeg-filters.html#signature)

### 5. Decode page identity over time

Choose the best *sequence* of pages rather than independently accepting the top
page for every frame. A practical dynamic-programming state model contains one
state per slide plus `unknown`, with transition costs such as:

- very low cost to stay on the current slide;
- low cost to move forward one page;
- higher but finite cost to move backward or jump;
- entry into `unknown` when the visual evidence is weak.

This is not merely a generic analogy. Schroth et al. model individual slides as
HMM states, include forward, backward, and false-transition hypotheses, and use
the Viterbi algorithm so ambiguous local decisions are resolved by subsequent
evidence. They explicitly note that presentation navigation can move forward or
backward and that a feature match can be invoked when state probabilities are
too similar.
[Schroth et al., *Synchronization of Presentation Slides and Lecture Videos Using Bit Rate Sequences*](https://web.stanford.edu/~bgirod/pdfs/Schroth_ICIP2011.pdf)

BeyondSlides need not reproduce that paper's codec-bit-rate observation model.
Its directly measured visual scores can serve as the state evidence while the
same temporal idea prevents one bad or occluded frame from creating a spurious
slide switch. A strictly monotonic decoder would be inappropriate because a
lecturer can revisit an earlier page.

This recommendation conflicts with the current `ARCHITECTURE.md` rule that the
transcript-window alignment path cannot move backward. The two outputs need not
be identical: video evidence can record the visibly presented page, including a
backward visit, while transcript-window alignment can retain its monotonic
approximation. Before connecting them, the project should decide explicitly
whether visual observations are allowed to revise that architectural rule;
silently discarding strong backward visual evidence would make the derived
timestamps less concrete.

### 6. Refine transition times

A one-Hz pass bounds a transition only to the interval between two samples. For
each state change, decode frames at a higher rate within that interval and find
the first stable run supporting the new page. Retain the original frame PTS as
the boundary evidence. This makes coarse processing cheap without limiting the
final timestamp to one-second precision.

## Alternatives and their roles

| Method | What it can establish | Recommended role |
| --- | --- | --- |
| Registered visual page matching | Which PDF page is visibly present at a particular video PTS | Primary evidence |
| Video scene/change detection | Where a visual transition may have occurred | Sampling and boundary-refinement hint only |
| OCR of the video frame | Text visible in the slide region | Fallback cue when geometry or image quality defeats visual matching |
| Transcript-to-slide retrieval | Slides semantically relevant to what is being said | Independent semantic evidence and sanity check, not concrete timing |
| Codec bit-rate sequence matching | Likely transitions and page sequence under suitable encoding assumptions | Interesting low-cost technique, but unnecessary for the first implementation |
| Learned image embeddings | Coarse visual similarity despite larger appearance changes | Later fallback if classical matching fails on representative recordings |

Learned embeddings are deliberately not in the first implementation. DINOv2,
for example, provides pretrained visual features intended to transfer across
image domains, which could make it a useful candidate generator when projector
appearance differs greatly from a PDF render. That invariance is not obviously
an advantage for distinguishing two nearly identical slide builds, however, and
model loading plus inference adds CPU and packaging cost to an identity-matching
problem that registered pixels and local features address directly. Evaluate an
embedding model only after labeled failures establish that this robustness is
needed.
[DINOv2 authors' repository and pretrained models](https://github.com/facebookresearch/dinov2)

OCR should remain secondary. Tesseract supports Simplified Chinese through the
`chi_sim` trained-data package, and its command-line interface accepts explicit
language selection.
[Tesseract installation and language documentation](https://tesseract-ocr.github.io/tessdoc/Installation.html)
[Tesseract command-line usage](https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html)
However, prior slide-synchronization work identifies low video resolution and
slides with little or no text as fundamental limitations of text-only matching.
[Schroth et al.](https://web.stanford.edu/~bgirod/pdfs/Schroth_ICIP2011.pdf)

## Failure modes and required behavior

### No slide is visible

Camera shots of the lecturer, black frames, demonstrations, or full-screen
videos must map to `unknown`. Carrying the previous page through these periods
may be a useful UI convenience, but it is an inference and should not be stored
as observed visual alignment.

### Occlusion, cursor movement, and annotations

A lecturer, pointer, subtitles, or live ink can cover part of the projected
slide. Use feature inliers distributed across the page, temporal agreement, and
possibly a mask for stable overlays. Do not require pixel equality.

### Perspective, zoom, blur, and projector color

Estimate a homography before SSIM; compare grayscale structure or edges; reject
homographies with too few inliers or implausible geometry. Persistent inability
to register the page should lower confidence rather than select the nearest
thumbnail.

### Builds and animations

The video may show an intermediate reveal while the PDF contains only a final
page, or the PDF may encode each reveal as a separate page. Consecutive pages can
therefore be nearly identical. Temporal decoding and score margins help, but the
output must preserve ambiguity when two references are visually
indistinguishable.

### Duplicate and near-duplicate pages

Visual evidence cannot uniquely identify two identical PDF pages. Presentation
order can select the likely occurrence, but that is an inferred slide position,
not an objective image match. Store a low margin or the tied candidates so later
transcript evidence or manual review can resolve them.

### Reordered or mismatched deck

The recording may use a different revision of the PDF. A sustained low score
across all pages should create an unknown interval and an import-level warning;
forcing every frame to one of the supplied pages would conceal the mismatch.

### Backtracking and jumps

Do not enforce strict monotonicity. Prefer ordinary forward progression while
allowing backward navigation and larger jumps at an explicit penalty. The
transition model should express presentation habits, not rewrite contrary frame
evidence.

### Layout changes and camera cuts

A fixed crop fails when the editor changes from slide-only to split-screen or a
camera view. Detect persistent layout changes and either re-estimate the slide
quadrilateral or enter `unknown`; scene-change evidence can trigger that work.

### Sampling uncertainty

A one-second coarse sample cannot prove the exact frame of a page change. Report
the bracketing sample timestamps until the local high-rate refinement has run,
and retain the refined frame PTS rather than deriving time from an array index.

## First evaluation

Before integrating visual alignment into the annotation pipeline, test it on
the real course as an inspectable development artifact:

1. Label the visible page and transition time for several short intervals,
   including one normal run, one occlusion, one build, one backward navigation
   if present, and one non-slide shot.
2. Measure page accuracy only where a supplied PDF page is actually visible.
3. Measure transition error in seconds separately from page accuracy.
4. Measure unknown detection and retain examples of high-confidence errors.
5. Compare independent per-frame winners with temporally decoded results.

This small labeled set should decide whether simple crop plus SSIM is sufficient
for this recording or whether homography-based registration is needed. It should
also determine thresholds; they should not be invented from the nominal ranges
of unrelated image-quality metrics.
