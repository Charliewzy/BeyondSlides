# Local application implementation

The first local-first application slice supports PDF + existing transcript
import, resumable analysis, live progress/token usage, and the interactive reader.
Video/audio transcription and shareable export are still being integrated.

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
2. Inspect the text preview and extraction warnings. Upload/preview does not
   contact the analysis provider.
3. Enter your OpenAI-compatible base URL, exact model name, and API key. Text
   is sent to that endpoint; original media is not attached to model requests.
   Advanced settings accept provider-specific request JSON. For GLM, disabling
   thinking can use `{"thinking":{"type":"disabled"}}`; no provider-specific
   option is imposed by default.
4. Start, monitor, stop after current work, or resume. Input/output token totals
   update on completed provider responses; partial availability is labeled.
5. Open the completed report with slides and optional recording playback.

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

HTTP implementation references: [Axum multipart uploads](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html)
and [Tower HTTP file serving](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
