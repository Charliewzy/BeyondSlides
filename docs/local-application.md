# Local application implementation

The local-first application supports PDF + existing transcript or recording
import, CPU transcription, resumable analysis, live progress/token usage, the
interactive reader, and shareable ZIP export. Broader transcript formats and
untimed-text handling are still being integrated.

## Launch

```sh
cargo run --release -- serve
# Or choose a data directory and port:
cargo run --release -- serve run/application 7842
```

Open `http://127.0.0.1:7842`. The server binds only to loopback; this is not a
multi-user hosted deployment. Poppler's `pdftotext` and `pdftoppm` must be on PATH,
and the existing dense-retrieval model may download on first use.

1. Import a slides PDF (up to 100 MiB) and normalized transcript JSON or FunASR
   timestamped TSV (up to 16 MiB). Optionally attach a browser-playable recording.
   Alternatively choose local CPU transcription and upload a recording instead
   of a transcript. Recordings are inspected with `ffprobe`; transcription
   requires an audio track. The whole upload is limited to 4 GiB.
2. Inspect the text preview and extraction warnings. Upload/preview does not
   contact the analysis provider.
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
There is no trustworthy percentage for this call, so the UI uses an
indeterminate stage. A graceful stop during transcription waits for that call
to finish and saves it before pausing. A hard interruption before the checkpoint
requires retranscribing; completed transcription is reused across resumes and
analysis-model changes. Fine-grained token timing is used when available,
otherwise playback retains explicitly coarser transcript-segment timing.

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
display, and a 390px layout without horizontal overflow. Existing
prototype-gated minimap behavior is unchanged.

`--recording <recording.wav>` on the verifier exercises real local ASR before
mock-model analysis. The 60-second real-lecture sample in
`run/application-asr-SexreZ/workspace/` produced 24 transcript segments and 267
timed tokens. CPU ASR including model loading took 28.5 seconds, and resuming
reused the exact transcription checkpoint. This is a short functional test, not
a whole-lecture speed estimate.

HTTP implementation references: [Axum multipart uploads](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html)
and [Tower HTTP file serving](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
