# Run local speech recognition natively with sherpa-onnx

## Status

Accepted.

## Context

Recording import previously launched a Python environment containing FunASR and
CPU PyTorch. That made a source checkout depend on a second package manager and
a large framework installation, complicated application packaging, and exposed
progress only through an adapter script. BeyondSlides still needs Chinese
recognition, voice activity detection, punctuation-bearing text, and fine-grained
timing on machines without a GPU.

## Decision

Use the sherpa-onnx Rust API with the INT8 SenseVoiceSmall model and Silero VAD.
FFmpeg remains responsible only for converting an uploaded recording to 16 kHz
mono PCM. The worker downloads pinned model assets on first use, verifies fixed
SHA-256 digests, caches them once per application data directory, and resumes
partial downloads. A file lock serializes concurrent model installation.

Detected speech regions retain one second of surrounding audio and overlapping
regions are merged before recognition. SenseVoice token timing becomes the
timed transcript when every recognized region supplies a complete valid timing
sequence; otherwise downstream playback uses transcript-segment timing. Existing
validated transcription checkpoints remain compatible and reusable.

The sherpa native runtime is statically linked. This avoids requiring users to
discover and ship an adjacent `libsherpa-onnx-c-api` shared library when running
the built BeyondSlides executable directly.

## Consequences

- Recording transcription no longer requires Python, PyTorch, FunASR, or a GPU.
- A first run downloads roughly 156 MiB and retains roughly 230 MiB of model
  files; later lectures reuse the cache.
- Model downloads and speech-region recognition expose native progress, and a
  graceful stop is observed between download increments, VAD chunks, and speech
  regions.
- Recognition output can differ from the previous FunASR pipeline because the
  VAD implementation and model export differ. The five-minute real-course smoke
  test is retained as an ignored, opt-in regression test rather than CI data.
- FFmpeg and ffprobe remain external runtime dependencies.
