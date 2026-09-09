# Local application implementation

The local-first application independently selects a written source and a
lecture source. It supports PDF or Rain Classroom slide import, existing
transcript or recording import, Rain Classroom recording import, CPU
transcription, resumable analysis, live progress/token usage, the interactive
reader, and shareable ZIP export. Transcripts can be timed subtitles or untimed
plain text; missing timestamps are never fabricated.

The sidebar's **查看示例报告** button opens a bundled real-lecture report in a
new tab, without importing a lecture, creating a job, or configuring a model.
The same self-contained file is checked in at `examples/demo/report.html` and
can be opened directly offline. It includes 80 slide images and reader
interactions, but deliberately omits the large recording and audio controls.
See `examples/demo/README.md` for provenance and regeneration instructions.

## Launch

The Linux x86-64 standalone bundle includes its own portable Chromium. Run its
`BeyondSlides` launcher to start the controller, open the application window,
and stop the controller when that window closes. Its persistent data is kept
outside the replaceable program directory; see [standalone.md](standalone.md).

For a source checkout:

```sh
cargo run --release -- serve
# Or choose a data directory and port:
cargo run --release -- serve run/application 7842
```

For an offline release layout, preinstall the verified native tools beside the
executable with `beyond-slides install-runtime-tools <directory>`. The expected
directory name is `runtime-tools` when it is executable-adjacent; normal source
runs use the operating-system cache automatically.

Open `http://127.0.0.1:7842`. Source runs bind to loopback by default; this is
not a multi-user hosted deployment. Docker and reverse-proxy configuration are
documented separately in [deployment.md](deployment.md). Pinned PDFium, FFmpeg,
and `ffprobe` assets are resolved beside the executable, from the user cache,
or downloaded with SHA-256 verification on first use. Dense-retrieval and Rain
Classroom OCR models may also download on first use.

1. Choose the written source: upload a slides PDF (up to 100 MiB), or select a
   Rain Classroom course, lecture, and presentation. Choose the lecture source
   independently: normalized transcript JSON, SRT, WebVTT, or UTF-8 plain text
   (up to 16 MiB), a local recording, or a Rain
   Classroom recording.
   Optionally attach a browser-playable recording.
   Alternatively choose local CPU transcription and upload a recording instead
   of a transcript. Recordings are inspected with `ffprobe`; transcription
   requires an audio track. The whole upload is limited to 4 GiB.
   A dedicated headless Chrome/Chromium profile renders the Rain Classroom
   login view; no separate provider window is shown. Provider-declared replay
   entries are downloaded and assembled into one `recording.mp4` before the
   unchanged transcription pipeline sees them; replay segmentation is not part
   of the UI or job model.
   Selected courseware pages are downloaded in provider order and assembled
   into the canonical `slides.pdf`. Native PP-OCRv5 recognizes the original
   page images on CPU, after which the PDFium importer retains
   extracted text where available and fills only sparse pages from OCR. The
   model cache is shared across lectures; no Python, PaddlePaddle, GPU, or OCR
   service is required.
   During import, recording progress uses downloaded bytes when sizes are
   known; courseware download and OCR progress use completed page count.
2. Inspect the text preview and extraction warnings. Upload/preview does not
   contact the analysis provider.
   “建议检查的页面” lists flagged pages in a horizontal thumbnail strip, with
   page numbers and warnings. Click to compare an enlarged page with its
   extracted text; arrow buttons, keyboard focus, and native scrolling support
   browsing. Review is optional and never blocks analysis. Existing imports
   use the same warning checks against saved slide text; no migration or model
   call is required. Only requested flagged pages are rendered, with two
   concurrent in-process PDFium renders at most.
   Preview PNGs live in a separate per-job `slide-review/` cache, not report
   assets or analysis checkpoints. Preview failures leave analysis available.
3. Choose either an OpenAI-compatible endpoint (base URL, exact model name,
   and API key) or a locally installed, authenticated Codex CLI. Codex models,
   reasoning efforts, and Fast availability are discovered from the signed-in
   account. Text is sent through that backend; original media is not attached
   to model requests. Codex mode reuses `codex login` and never receives the
   API key field.
   Advanced settings accept provider-specific request JSON. For GLM, disabling
   thinking can use `{"thinking":{"type":"disabled"}}`; no provider-specific
   option is imposed by default. Adaptive scheduling separately exposes its
   starting concurrency and upper limit; changing either remains operational
   and does not invalidate validated checkpoints.
4. Start, monitor, stop after current work, or resume. Input/output token totals
   update on completed provider responses; partial availability is labeled.
5. Open the completed report with slides and optional recording playback, or
   download the ZIP. Extract the entire ZIP before opening `report.html`; keep
   `report.assets` beside it. The ZIP contains lecture text, slides, and attached
   recording, not private model traces or provider settings. Share with permission.

Plain text is split into bounded ingestion units before restoration; these are
not the final semantic passages. Untimed inputs use character-based windowing.
JSON timing may also be absent: every segment must either supply both
`start_ms`/`end_ms`, or every segment must omit both (or set both to null).
Existing numeric JSON timestamps are unchanged. Untimed inputs can produce the
full text/slides reader, but passage-linked playback is unavailable even if a
recording is attached. The export does not include that unused recording.
SRT and WebVTT use the [subtp parsers](https://docs.rs/subtp/latest/subtp/);
subtitle sequence labels are normalized into contiguous source IDs.

Rain Classroom authentication stays in a dedicated Chrome/Chromium profile at
`.rain-classroom-browser` inside the application data directory. Course and
lecture discovery run in that authenticated page context. Selecting either Rain
source silently checks the saved session: a valid session populates courses and
shows “已登录雨课堂”; an expired session offers the in-app QR dialog. Graceful
controller shutdown closes Chromium so it can flush this profile. The browser
adapter prefers the verified executable adjacent to a standalone binary and
otherwise uses chromiumoxide's system-browser discovery. The AppImage uses
extract-and-run mode for headless provider automation so FUSE is not required.
The browser submits only the selected classroom, lecture, and presentation identifiers to
the local backend; the backend obtains fresh, short-lived asset URLs and never
accepts signed URLs from the UI. This adapter targets Rain Classroom's current
private web interface and may require maintenance when the site changes.

## Local CPU transcription setup

Managed `ffmpeg` and `ffprobe` executables handle media conversion and probing.
The Rust worker uses
the sherpa-onnx runtime with INT8 SenseVoiceSmall and Silero VAD directly; it
does not require Python, PyTorch, FunASR, or a GPU.

On first use, BeyondSlides downloads an approximately 156 MiB model archive and
the VAD model into `<application-data>/models/`. Downloads resume after an
interruption, and fixed SHA-256 digests are checked before any model is loaded.
The extracted cache is approximately 230 MiB and is shared by every lecture in
that application data directory. A file lock prevents two workers from
installing the same model concurrently.

The UI distinguishes audio extraction, model download/loading, speech detection,
recognition, and finalization. Recognition progress is the duration of completed
detected speech regions divided by their total duration, with region counts
shown alongside it. Silence is excluded, so this is work completed rather than
a chronological playback position. One second of context is retained around
detected regions and overlapping padded regions are merged to avoid clipping
quiet boundary syllables without duplicating speech. A graceful stop is checked
between regions; a hard interruption before the atomic checkpoint requires
retranscribing. Completed transcription is reused across resumes and
analysis-model changes. Fine-grained SenseVoice token timing is used when
complete; otherwise playback retains explicitly coarser transcript-segment
timing.

The **Debug logs and stage timings** panel exposes the processing-worker log,
including native transcription diagnostics, and retains access to legacy
Python-transcription logs from old runs. It polls while expanded, displays the most recent
128 KiB as plain text, and pauses the displayed output when you scroll upward
or uncheck follow. Full sanitized logs can be downloaded. Model request/response
traces are not exposed here; logs can still contain lecture content and local
paths, so inspect them before sharing.

Configured API keys (including JSON-escaped and URL-encoded forms) are redacted
across pipe-read boundaries before browser-visible logs are saved. A lightweight
worker supervisor owns the log pipes independently of the controller, preserving
processing and log capture across server restarts.
Workers started by this version also use a separate Unix process group, so a
terminal Ctrl+C sent to the controller does not interrupt their processing. Old
raw `worker.log`/`asr.log` files are never served; new captures use
`worker-debug.log` and `transcription/asr-debug.log`. This is credential filtering,
not a guarantee that arbitrary secrets printed by third-party code are removed.

Stage timings include audio extraction, speech detection, recognition, and
finalization. Completed timing values appear in debug details; reused
transcription results label their original model timings as historical. Existing
transcription checkpoints remain reusable without rerunning ASR just to obtain
telemetry. Instrumentation observes existing inference calls without changing
their audio inputs, segmentation, text, or timestamp processing.

The capture uses [Tokio subprocess pipes](https://docs.rs/tokio/latest/tokio/process/struct.Command.html)
and [strip-ansi-escapes](https://docs.rs/strip-ansi-escapes/latest/strip_ansi_escapes/fn.strip.html)
for readable terminal output, rather than treating terminal text as executable HTML.

Implementation references: [sherpa-onnx Rust API](https://docs.rs/sherpa-onnx/),
[SenseVoice](https://github.com/FunAudioLLM/SenseVoice), and
[ZIP writer](https://docs.rs/zip/latest/zip/write/struct.ZipWriter.html).

Keys are passed only to the local backend and worker environment, not saved in
job metadata or browser storage. Re-enter the key when resuming. Private source
files, model traces, and worker logs remain in the application data directory;
only the generated report and report assets have HTTP file-serving routes.

The job owns its worker independently of the browser. A server restart also
leaves an existing worker running; an OS file lock prevents starting a duplicate.
A worker that exits without a recorded outcome is shown as interrupted, never
assumed to have completed. Restart the server against the same data directory.
Worker elapsed time is checkpointed once per second. After a hard interruption,
the UI shows the last recorded lower bound rather than counting time spent
offline; a subsequent resume carries that recorded work time forward.

Each pipeline stage also checkpoints cumulative active elapsed time once per
second. Completed durations remain visible; paused/offline time is excluded.
Measurable stages show remaining time directly from indicatif's `ProgressBar::eta`
and the corresponding elapsed-plus-remaining stage total. The estimator only
sees newly completed work after the checkpoint baseline, never restored counts.
It restarts on resume; no ETA appears until new work completes. Indeterminate
preparation and model-loading substages have no invented end-to-end ETA.

During speech recognition, the controller feeds observed completed/total speech
duration into indicatif and returns a separate recognition-only ETA. This is a
read-only projection of `asr-progress.json`, so an already-running older worker
can benefit after the controller is restarted. The first observation establishes
a baseline; an estimate appears after further speech progress arrives. The
controller's estimator is shared across browser polls and resets on controller
restart, a new attempt, counter regression, changed total, reuse, inactivity, or
leaving recognition. It never writes back to worker files. The UI labels this
estimate “预计语音识别剩余” and explicitly excludes final checkpoint saving;
it does not add it to cumulative stage time as an end-to-end forecast.

Reused stages are labeled. A hard kill may lose up to the last timing heartbeat;
old progress files without timings show “耗时未记录” rather than fabricated
historical durations. A worker already running older code will not acquire the
worker-side stage telemetry merely because the controller is rebuilt. The
read-only recognition ETA above is the exception: it uses observations older
workers already publish.

New debug captures have readable GMT start timestamps and explicitly identify
subprocess output, not model retries. The UI translates old numeric markers for
display without rewriting old logs. Sparse PDF text warnings group affected
pages, retain extracted character counts, and explain that title/image pages
can legitimately contain little extractable text. Old job warnings are also
translated at display time without changing saved source metadata.

Scheduling changes reuse the same validated checkpoints. Changes to provider,
model, or request options require confirmation and create a new run revision.
Jobs created with the retired window-owned passage preparation path remain
readable. Starting one under the standard global-boundary pipeline creates a
new revision, copies reusable restoration artifacts, and revalidates them; old
analysis files are not adopted. Prior run revisions remain on disk. The initial
UI links the latest revision's report.

## Worker control

An `analyze` worker can receive `BEYOND_SLIDES_WORKER_CONTROL`, a private local
directory owned by the application. No behavior changes for ordinary CLI runs
without this setting.

- `progress.json` is atomically replaced with structured stage progress. A null
  total means planning or unquantified local work, not zero remaining work.
- Creating `stop-requested` stops new window/batch admission. Already-admitted
  conversations can finish tool calls, retries, and repairs and save results.
- Resume uses a fresh worker and removes the stop marker first. Existing model
  checkpoint validators remain authoritative; progress is never a checkpoint.
- Errors also stop admission and drain other successful work, retaining the
  first failure for diagnosis instead of cancelling checkpointable siblings.

The worker-control directory must not be placed inside an uninitialized pipeline
run directory: existing run manifests intentionally reject unrecognized files.
The application will use separate sibling `control/` and `analysis/` directories.

## Verification

`uv run scripts/verify_local_application.py run/new-application-verification`
exercises the actual built `target/debug/beyond-slides` server and workers using
a deterministic local mock provider. It checks import, graceful stopping,
checkpoint-preserving resume, restarting the controller while work continues,
token totals, settings-change confirmation, report/media serving, and local
request restrictions. It requires the managed native runtimes and cached dense model; it does
not make paid model requests. Its synthetic decisions test plumbing, not model
quality. The fixture recording is only a playback-resource smoke test, not
aligned lecture evidence.

The first verified run is retained in `run/application-verification-20260906/`.
Browser checks also covered import, reload/reconnection, report opening, token
display, and a 390px layout without horizontal overflow. Reports with slides
show the middle binary minimap by default on wide screens; no prototype query
parameter is needed. It remains hidden at widths of 70rem or less. Previously
generated reports must be re-rendered to pick up template changes (no model
inference is required).

`--recording <recording.wav>` on the verifier exercises real local ASR before
mock-model analysis. The 60-second real-lecture sample in
`run/application-asr-SexreZ/workspace/` produced 24 transcript segments and 267
timed tokens. CPU ASR including model loading took 28.5 seconds, and resuming
reused the exact transcription checkpoint. This is a short functional test, not
a whole-lecture speed estimate.

`--untimed` tests plain-text import through the complete analysis/report/export
workflow and verifies that no zero-valued timestamps or audio links are invented.

The final MP4 check is retained under `run/application-video-JMA7Kd/verified/`.
A 60-second speech excerpt produced 31 source segments and 318 timed tokens;
CPU transcription including model loading took 33.3 seconds in that run. The
test stopped during transcription, verified that its result was saved before
pausing, resumed without retranscribing, and completed both passage-preparation
modes while preserving the first report and reusing restoration. Model analysis
in these application tests uses a deterministic mock: this verifies integration,
not the semantic quality of a fresh real-provider run.

The completed timed-input/settings-revision verification is in
`run/application-reprocessing-final-20260906/`; the untimed path is in
`run/application-untimed-verification-20260906/`. To repeat browser checks against
one of those completed verifier workspaces:

```sh
uv run scripts/verify_application_browser.py run/application-untimed-verification-20260906
# If needed, pass --chromium /path/to/an/existing/chromium/executable.
```

The browser check requires Playwright's Chromium (or the explicit executable),
imports one additional small test lecture into the specified workspace, and
saves desktop/mobile screenshots there. It checks report/slide loading, ZIP
download, reload/reconnection, source-mode controls, and untimed upload without
contacting a model provider.

Transcription visibility and log checks are retained in
`run/transcription-observer-uzHolS/`. The instrumented 60-second recording
produced exactly the same normalized transcript and timed tokens as the prior
uninstrumented checkpoint. The application test additionally verifies measured
ASR substages/timings, stopping during model loading, reuse after resume,
sanitized log endpoints/downloads, and log availability after a controller
restart. Browser tests cover weighted recognition percentages, indeterminate
model loading, paused progress, log-source selection, escaping, and scrolling
to pause/follow the bounded log display.

HTTP implementation references: [Axum multipart uploads](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html)
and [Tower HTTP file serving](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
