# Local application implementation

The local-first application supports PDF + existing transcript or recording
import, CPU transcription, resumable analysis, live progress/token usage, the
interactive reader, and shareable ZIP export. Transcripts can be timed subtitles
or untimed plain text; missing timestamps are never fabricated.

## Launch

```sh
cargo run --release -- serve
# Or choose a data directory and port:
cargo run --release -- serve run/application 7842
```

Open `http://127.0.0.1:7842`. The server binds only to loopback; this is not a
multi-user hosted deployment. Poppler's `pdftotext` and `pdftoppm` must be on PATH,
and the existing dense-retrieval model may download on first use.

1. Import a slides PDF (up to 100 MiB) and normalized transcript JSON, FunASR
   timestamped TSV, SRT, WebVTT, or UTF-8 plain text (up to 16 MiB).
   Optionally attach a browser-playable recording.
   Alternatively choose local CPU transcription and upload a recording instead
   of a transcript. Recordings are inspected with `ffprobe`; transcription
   requires an audio track. The whole upload is limited to 4 GiB.
2. Inspect the text preview and extraction warnings. Upload/preview does not
   contact the analysis provider.
   “建议检查的页面” lists flagged pages in a horizontal thumbnail strip, with
   page numbers and warnings. Click to compare an enlarged page with its
   extracted text; arrow buttons, keyboard focus, and native scrolling support
   browsing. Review is optional and never blocks analysis. Existing imports
   use the same warning checks against saved slide text; no migration or model
   call is required. Only requested flagged pages are rendered, with two
   concurrent Poppler renders at most and a 30-second rendering timeout.
   Preview PNGs live in a separate per-job `slide-review/` cache, not report
   assets or analysis checkpoints. Preview failures leave analysis available.
3. Enter your OpenAI-compatible base URL, exact model name, and API key. Text
   is sent to that endpoint; original media is not attached to model requests.
   Advanced settings accept provider-specific request JSON. For GLM, disabling
   thinking can use `{"thinking":{"type":"disabled"}}`; no provider-specific
   option is imposed by default.
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

## Local CPU transcription setup

`ffmpeg`, `ffprobe`, Python, FunASR and CPU PyTorch are needed only for recording
transcription (ffprobe also inspects optional uploaded recordings). On a fresh
Linux checkout, one setup path is:

```sh
uv venv --python 3.11 .venv  # only if this environment does not already exist
uv pip install --python .venv/bin/python torch torchaudio --index-url https://download.pytorch.org/whl/cpu
uv pip install --python .venv/bin/python funasr==1.4.5
```

The worker uses the checkout's `.venv` if available, otherwise `python3`. To use
another environment, set `BEYOND_SLIDES_ASR_PYTHON` to its Python executable when
launching the server. This is server configuration, not an uploaded command.

Transcription uses SenseVoiceSmall + FSMN VAD + punctuation, always on CPU. The
first use may download model weights; recording contents remain local. It runs
one complete recording inference and saves an atomic validated checkpoint.
The UI distinguishes audio extraction, model loading, speech detection,
recognition, and finalization. Recognition progress is the duration of completed
detected speech regions divided by their total duration, with region counts
shown alongside it. Silence is excluded; FunASR can process regions in length
order, so this is work completed, not a chronological playback position or time
remaining. Other substages remain indeterminate. If the library's observed
batch structure is unsupported, recognition also remains indeterminate rather
than inventing a percentage. A graceful stop during transcription waits for that call
to finish and saves it before pausing. A hard interruption before the checkpoint
requires retranscribing; completed transcription is reused across resumes and
analysis-model changes. Fine-grained token timing is used when available,
otherwise playback retains explicitly coarser transcript-segment timing.

The **Debug logs and stage timings** panel exposes separate processing-worker
and Python-transcription logs. It polls while expanded, displays the most recent
128 KiB as plain text, and pauses the displayed output when you scroll upward
or uncheck follow. Full sanitized logs can be downloaded. Model request/response
traces are not exposed here; logs can still contain lecture content and local
paths, so inspect them before sharing.

Configured API keys (including JSON-escaped and URL-encoded forms) are redacted
across pipe-read boundaries before browser-visible logs are saved. A lightweight
worker supervisor owns the log pipes independently of the controller, preserving
processing and log capture across server restarts. Python runs unbuffered.
Workers started by this version also use a separate Unix process group, so a
terminal Ctrl+C sent to the controller does not interrupt their processing. Old
raw `worker.log`/`asr.log` files are never served; new captures use
`worker-debug.log` and `transcription/asr-debug.log`. This is credential filtering,
not a guarantee that arbitrary secrets printed by third-party code are removed.

Stage timings include model loading, speech detection, recognition, punctuation,
and saving/finalization. Completed timing values appear in debug details; reused
transcription results label their original model timings as historical. Existing
transcription checkpoints remain reusable without rerunning ASR just to obtain
telemetry. Instrumentation observes existing inference calls without changing
their audio inputs, segmentation, text, or timestamp processing.

The capture uses [Tokio subprocess pipes](https://docs.rs/tokio/latest/tokio/process/struct.Command.html)
and [strip-ansi-escapes](https://docs.rs/strip-ansi-escapes/latest/strip_ansi_escapes/fn.strip.html)
for readable terminal output, rather than treating terminal text as executable HTML.

Implementation references: [FunASR CPU/SenseVoice usage](https://github.com/modelscope/FunASR)
and [ZIP writer](https://docs.rs/zip/latest/zip/write/struct.ZipWriter.html).

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
It restarts on resume; no ETA appears until new work completes. Preparation and
whole-call CPU transcription have no invented ETA. Speech-recognition progress
still reports its measured work percentage separately.

Reused stages are labeled. A hard kill may lose up to the last timing heartbeat;
old progress files without timings show “耗时未记录” rather than fabricated
historical durations. A worker already running older code will not acquire the
new telemetry merely because the controller is rebuilt.

New debug captures have readable GMT start timestamps and explicitly identify
subprocess output, not model retries. The UI translates old numeric markers for
display without rewriting old logs. Sparse PDF text warnings group affected
pages, retain extracted character counts, and explain that title/image pages
can legitimately contain little extractable text. Old job warnings are also
translated at display time without changing saved source metadata.

Scheduling changes reuse the same validated checkpoints. Changes to provider,
model, or request options require confirmation and create a new run revision.
Changing passage preparation alone copies restoration artifacts into the new
revision and revalidates them; old analysis files are not adopted. Prior run
revisions remain on disk. The initial UI links the latest revision's report.

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
request restrictions. It requires Poppler and the cached dense model; it does
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
