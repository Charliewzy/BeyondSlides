# Local application implementation

The local-first application is under construction. This document records the
worker contract; a browser launch command will be added with the server/UI slice.

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

## Planned interface

Upload a PDF plus either a transcript or recording, inspect a source preview,
configure the provider, and start a persistent job. The processing view exposes
stage counts, elapsed time, actual provider-reported token usage, and graceful
stop/resume. A finished job opens the existing interactive reader and supports
an export containing only the report and its assets, never keys or model traces.

The server will bind to loopback, reject cross-origin mutations, stream uploads
to generated filenames rather than trusting client paths, and keep API keys out
of saved job metadata. Existing prototype-gated minimap behavior is unchanged.

HTTP implementation references: [Axum multipart uploads](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html)
and [Tower HTTP file serving](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
