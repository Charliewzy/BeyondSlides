# Support Codex through its local app-server protocol

BeyondSlides supports two model backends behind `LectureModelBackend`:
the existing OpenAI-compatible Chat Completions adapter and a local Codex
app-server adapter. The lecture pipeline depends on validated operations
(restoration, annotation, boundary classification, and comparison), not on a
provider's message or tool-call representation.

Codex is launched once, lazily, as `codex app-server --listen stdio://` and its
newline-delimited protocol is multiplexed across ephemeral task threads. It
reuses the user's cached `codex login` authentication, so BeyondSlides neither
asks for nor stores a ChatGPT credential. Each turn supplies a JSON Schema and
the same deterministic validation and repair loop used by other backends.
Usage notifications feed the existing token diagnostics; requests and
responses use the existing redacted model trace.

Lecture analysis is a text transformation, not a coding task. Codex therefore
runs in an empty temporary working directory with a read-only sandbox,
`approvalPolicy: never`, disabled built-in tool feature flags, explicit
instructions forbidding tools, and a host-side failure if a tool item
nevertheless appears. The process does not inherit `BEYOND_SLIDES_API_KEY`.
No lecture files are placed in its working directory; only serialized task text
is sent. The first real task remains the canary for model availability and
structured-output behavior.

App-server is a versioned protocol. BeyondSlides validates the handshake and
every response shape at runtime, keeps one ignored live integration test for
authenticated development machines, and leaves normal CI independent of Codex
installation or subscription quota. Backend identity is part of restoration,
passage, and comparison checkpoint identity so results are never silently
reused across adapters. Manifests written before this decision default a
missing backend field to `openai_compatible`, preserving their existing
checkpoint compatibility.
