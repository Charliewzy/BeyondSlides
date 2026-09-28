# Release-readiness review — 2026-09-06

## Public-release follow-up — 2026-09-28

The course instructor has granted the project author permission to distribute
the bundled real-course example publicly with BeyondSlides. `NOTICE`, the main
README, and the example README record that permission while keeping the course
material outside the project's MIT software license. Replacing the example or
rewriting its Git history is therefore no longer a public-release prerequisite.

The public-release preparation was repeated from commit `2859258` in a fresh
clone. Formatting, strict Clippy, the complete default Rust test suite, 18
lightweight Python tests, and the release build passed. The local mock-provider
application verifier completed a real server/worker run with 12 model calls;
the browser verifier passed reload, debug-log, report, export, mobile, source,
and untimed-import checks without JavaScript errors.

Gitleaks 8.30.1 scanned every reachable commit and the repository's existing
GitHub Actions log with full redaction enabled and reported no findings. The
repository has no configured GitHub Actions secrets. This scan supplements,
rather than replaces, GitHub's automatic secret scanning after publication.

The rebuilt `beyond-slides_0.1.0-1_amd64.deb` is 222 MiB (967 MiB installed).
Its package verifier passed the layout, AppArmor, launcher, managed runtime,
Chromium, HTTP, and four-server Rain Classroom smoke checks. Its SHA-256 digest
is `055da9cce18b7a63b6f4dde261f0c99e2840365c3e1eaa38fa9a2423b8f9e6e7`.

## Native ASR follow-up — 2026-09-08

Recording transcription now runs through statically linked sherpa-onnx with
INT8 SenseVoiceSmall and Silero VAD. Python, PyTorch, and FunASR are no longer
runtime requirements, and the former FunASR TSV compatibility path has been
removed. First-run model assets are resumable, SHA-256 verified, protected by
an installation lock, and cached outside lecture directories. The browser
reports model-download bytes and speech-region recognition progress.

The complete strict Rust suite and 18 remaining lightweight Python tests pass.
A release-mode five-minute native ASR smoke test produced 45 punctuated
transcript segments with valid non-overlapping token timing in 9.35 seconds. A
separate clean-cache application-worker smoke test downloaded, verified, and
extracted all 156 MiB of compressed model assets, then processed a 60-second
recording through the persisted checkpoint seam. It produced 11 transcript
segments and 294 timed tokens in about 32 seconds overall. A directly launched
78 MiB release executable has no dynamic Sherpa or ONNX Runtime dependency.
The resumable-download unit test also starts from a partial file and verifies
atomic publication; abrupt process termination during a real remote transfer
remains a useful future fault-injection test.

## Course-submission follow-up

The three code findings below have now been addressed in separate commits:

- `6d04f42`: configured credential filtering at trace persistence, including
  successful raw responses and provider errors; returned provider errors are
  filtered before truncation too. Regression tests cover plain and URL-encoded
  echoes with a local mock provider. Historical traces remain sensitive.
- `dcd44fd`: same-destination audio inputs are preserved; replacements are
  staged before publication/cleanup. Tests cover repeated and equivalent-path
  rendering, missing replacements, extension changes and original preservation.
- `b00b745`: exclusive lifetime-held CLI run locks, including nested restoration;
  tests cover competing processes, initialization and lock release on drop.

The project owner selected MIT for the software and explicitly retained the
bundled example. `NOTICE` excludes third-party course content from MIT and now
records the instructor's permission to distribute the example publicly with
BeyondSlides. Minimal CI covers Rust formatting, Clippy, Rust tests and
lightweight Python tests, using Rust 1.94.0 / Python 3.11. Native test dependencies
and the Python tests' `httpx` dependency are explicitly installed.

The remainder records the **original review snapshot**, not the current status
of those fixed findings. The target is a Linux/WSL local course submission, not
an unrestricted public or multi-user hosted production release. A clean-machine
ASR installation and broader platform/resource fault testing remain future
release work. CI itself cannot be observed on GitHub until the commits are pushed.

Follow-up verification on Rust 1.94.0 passed formatting, strict Clippy and the
complete all-target Rust test suite (including the new regressions). All 26
Python tests passed in an isolated Python 3.11 environment with only their
declared `httpx` dependency. A fresh mock-provider application run completed with
17 requests, exercising stop/resume, controller restart and export. Separately,
the 60-second local CPU ASR sample completed in 24.8 seconds with FunASR 1.4.5,
producing 24 source segments and 267 timed tokens. These results use this machine's
model caches and do not constitute a clean-machine installation test.
The browser verifier also passed desktop/mobile, demo, reader, log, reload and
export checks against that fresh application workspace.
Artifacts are under `run/release-final-smnPZm/`; no paid inference was performed.

## Verdict and scope

**Do not label this version production-ready yet.** The local course-project
preview works end to end, but this review found reproducible credential-trace
and recording-deletion bugs, plus a CLI concurrency safety gap. Public release
also needs an explicit licensing and demo-redistribution decision.

Reviewed code snapshot: `48b940c`. Previous `main` was
`01dc2369ef3c6ba488e8907930f6728cc7e348c8`; `main` was fast-forwarded to the
reviewed snapshot without rewriting the commit stack. The README and this review
are documentation added afterward. Nothing was pushed, tagged or published.

The code-review skill split review into independent Standards and Spec axes.
The fixed diff was `git diff 01dc2369...48b940c`. Sources were `AGENTS.md`,
`CONTEXT.md`, relevant ADRs, `ARCHITECTURE.md`, `docs/local-application.md` and
`docs/running.md`. Additional release checks covered build/dependency setup,
packaging, source/media safety, and actual server/browser workflows. No separate
coding-standards document or originating GitHub issue was identified.

This is a repository-wide release assessment, not a formal security audit or a
claim that every code path was exercised. Production code was not changed to fix
these findings during the review.

## Standards

### S1 — P1: provider responses can persist the configured API key

**Documented-contract violation.** `ARCHITECTURE.md`, section 6, says “API keys
and authorization headers are never recorded.” `ModelExchangeTrace` documents
the same promise.

In [chat_completions.rs](../src/chat_completions.rs), `chat` records raw successful
responses around lines 579–587 and records provider errors around lines 610–619.
`model_provider_error` / `model_web_error` retain the error body and message
around lines 1396–1449 without filtering configured credentials. The application's
debug-log sanitizer does not sanitize these private trace writes.

**Reproduced:** an isolated loopback mock endpoint received a dummy API key from
the subsequently removed `restore-canary` diagnostic command.

- A 401 response containing the dummy key stored it verbatim in a
  `provider_error` trace event; the command exited with failure.
- A valid 200 restoration response with an extra
  `provider_debug.echoed_key` field stored it verbatim in a `response` trace event;
  the command succeeded.

No real credential or model endpoint was used. This does not establish that any
real provider has echoed a user's key. It does establish that the documented
guarantee is not enforced.

**Required before release:** filter configured credentials at a shared trace
persistence boundary, covering requests/options, parsed and raw responses,
errors and diagnostic messages. Test plain and encoded credential forms while
retaining useful non-sensitive diagnostics. Existing traces will not become
safe merely by fixing future writes; treat them as sensitive local files.

### S2 — P2: ordinary CLI runs do not enforce a single writer

**Correctness / design judgment, not an explicit ADR breach.** The web application
has controller/job locks, but ordinary CLI `restore` / `analyze` invocations
initialize run directories and load checkpoints without a lifetime-held exclusive
run lock. See [restoration_run.rs](../src/restoration_run.rs), lines 103–114,
and [run_support.rs](../src/run_support.rs), lines 248–302.

Two processes targeting one incomplete run can load the same checkpoint slots,
issue duplicate paid requests, replace each other's results, and independently
reconstruct trace IDs. A second trace opener also has authority to trim a partial
tail while another writer is active. `ModelExchangeTrace`'s mutex protects clones
inside one process, not independent opens/processes.

This finding is based on code-path inspection; this review did not run a
two-process overwrite race.

**Required before release:** acquire an OS-backed run lock before initialization
or checkpoint/trace mutation and hold it through completion, accounting for the
nested restoration directory. Test rejection of a second writer and release of
the lock after a crash. Meanwhile, run only one mutating CLI process per output
directory, and never point the CLI at a live application job's analysis directory.

Standards summary: **2 findings**; the worst is the reproduced credential
persistence violation. Cosmetic smell refactors were not treated as blockers.

## Spec

### F1 — P1: re-rendering can delete the supplied recording

The run-layout specification says supplied recordings are made available in
`report.assets`, normally by hard link (`ARCHITECTURE.md`, section 7).
`docs/running.md` promises that assets contain the audio and remain alongside
the report for sharing.

In [report_assets.rs](../src/report_assets.rs), `prepare_audio_asset` checks that
the input exists, then calls `remove_stale_audio_assets` at line 49 **before**
linking or copying that input. If `--audio` already refers to
`<report-stem>.assets/lecture-audio.<extension>` for the output report, cleanup
deletes the supplied file. The subsequent hard-link and copy both fail.

**Reproduced:** used the release binary, valid mock-generated lecture analysis,
and a disposable `report.assets/lecture-audio.flac` fixture. Called
`render-analysis ... report.html --audio report.assets/lecture-audio.flac`.
The command failed with `No such file or directory`, and the input no longer
existed afterward. No real recording was touched. If that asset were the only
remaining copy of a recording, this would be data loss.

**Required before release:** recognize input/destination identity before cleanup,
stage replacements safely, and remove obsolete assets only after the new asset
is secured. Regression tests must cover same-path input, equivalent paths,
failed copy/link, extension changes, and preserving the previous working report.
Until fixed, keep the original recording outside the output asset directory.

No additional independently substantiated blockers were found in the inspected
restoration ownership, source projection, comparative checkpoint identities,
transcript imports, or ordinary application stop/resume paths. Passing schema and
coverage checks does not prove that LLM judgments are semantically correct.

Spec summary: **1 finding**; the worst is reproduced deletion of supplied audio.

## Release engineering and distribution

These are release gates / validation gaps, not additional code-review-axis findings.

1. **Decide licensing and demo rights.** No project `LICENSE`/`COPYING` file or
   Cargo license metadata is present. The embedded real-course report includes
   lecture text and 80 original slide images. Attribution is present; documented
   authorization for public redistribution was not found. The owner should
   choose a software license and confirm permission for the bundled course
   material, or replace it with an explicitly redistributable example. This
   review does not choose a license or assert that permission is absent.
2. **Make the tested environment reproducible.** `Cargo.lock` is present, but
   there is no declared minimum Rust version or pinned toolchain. ASR setup pins
   FunASR but not the full PyTorch/Python dependency graph or downloaded model
   revisions. A fresh-machine recording-to-report run is needed with a recorded
   dependency set; the current machine's caches are not a clean-install test.
3. **Add automated release checks.** No checked-in `.github/workflows/` jobs
   were found. Automate formatting, lint, unit/integration tests, dependency
   audits, and a Linux mock-provider application smoke test. Keep model-download
   and real-provider quality checks explicit rather than silently skipping them.
4. **Declare the supported distribution target.** The current verified path is
   Linux/WSL from source with Poppler, FFmpeg, Rust and optional Python/ASR setup.
   There are no checked-in installers or release workflows. Do not advertise a
   standalone cross-platform desktop product yet. The server's loopback-only,
   single-user scope is intentional; it is not ready for public multi-user hosting.
5. **Bound local input/resource failures before broad distribution.** Upload-byte
   limits exist, but full PDF extraction/rendering and `ffprobe` subprocesses
   lack the thumbnail renderer's timeout, and there is no complete disk quota or
   job-retention policy. Test damaged/very large files, low disk space, missing
   tools, interrupted model downloads, and cancellation during these stages.
   These paths were inspected, not exhaustively fault-injected in this review.

## Verification performed

All commands below completed successfully unless noted. No paid provider calls
or full real-lecture inference were made, and no user worker was stopped.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --locked --all-targets --all-features -- --test-threads=1` | Passed; download-dependent dense test is ignored by default |
| `cargo test --locked --test retrieval -- --ignored --test-threads=1` | Passed the dense Chinese retrieval test using cached weights |
| `cargo +stable check --locked --all-targets` | Passed with Rust 1.94.0 |
| `cargo build --release --locked` | Passed with Rust 1.97.0-nightly |
| Python script unit tests | 26 passed with Python 3.11 |
| `uv pip check --python .venv/bin/python` | Installed environment consistent; not a Python vulnerability audit |
| Timed transcript end-to-end verifier | Release binary, 17 local mock requests, completed |
| Untimed transcript end-to-end verifier | Release binary, 17 local mock requests, completed |
| Browser verifier | Passed desktop/mobile, reload, logs, previews, demo, reader and export checks |
| Cargo package file-list inspection | Includes templates, prompts, Python bridge and embedded demo; missing package metadata warning |
| Narrow credential-pattern scan | No matches in working files or reachable commit diffs for the tested proxy/key/private-key patterns; not a comprehensive secret scan |

The application verifiers exercise actual server/worker processes, cooperative
stop, checkpoint reuse, controller restart, settings-change confirmation, token
telemetry, report/media serving and ZIP export. Their model judgments are
deliberately synthetic. The browser verifier uses the debug binary and Chromium;
the separate end-to-end verifiers above use the release binary.

`cargo-audit 0.22.2` checked 418 locked dependencies against RustSec database
commit `5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5` (updated 2026-09-02):
**0 known vulnerabilities**, **1 unmaintained dependency warning** for
`paste 1.0.15` (`RUSTSEC-2024-0436`), pulled through `genai` and
`fastembed → tokenizers`. This is a maintenance warning, not a reported exploit.
It does not audit native Poppler/FFmpeg/ONNX binaries, Python packages, or model
weights. The audit utility was installed in an isolated temporary directory,
not added as an application dependency.

Local evidence retained for this review session:

- `/tmp/beyond-slides-release-check-bowbg9/timed/` — timed run and browser screenshots
- `/tmp/beyond-slides-release-check-bowbg9/untimed/` — untimed run
- `/tmp/beyond-slides-release-check-bowbg9/audio-repro/` — disposable audio-deletion reproduction
- `/tmp/beyond-slides-trace-review-q0kRml/probe.py` — dummy-key mock-provider reproduction and adjacent traces

These paths are ephemeral, are not repository dependencies, and may be removed
by normal temporary-directory cleanup. Verification scripts are checked in under
`scripts/` so application checks can be repeated with a new output directory.

## Recommended next milestone

Fix S1, S2 and the audio-deletion finding in separate regression-tested commits;
then resolve licensing/demo distribution and add the reproducible Linux release
check. Run a fresh-environment CPU recording import and a small explicitly
authorized real-provider smoke run before promoting the local preview to a
release candidate. Re-run this review's checks against that exact candidate
commit.
