# ADR 0012: Package local Docker deployments

## Status

Accepted

## Context

BeyondSlides previously bound the application server to `127.0.0.1` and
accepted only a `Host` header containing its internal listening port. A Docker
port mapping cannot reach a process bound only to the container loopback
interface, and the external `Host` port correctly differs from the internal
port for mappings such as `127.0.0.1:80:7842`.

The application also needs PDFium, FFmpeg, and `ffprobe`, but requiring those
programs to be installed independently makes a clean runtime image difficult to
reproduce.

## Decision

The source application continues to bind to loopback by default. Deployments
may set `BEYOND_SLIDES_BIND_ADDRESS`; the Docker image sets it to `0.0.0.0` and
expects the operator to restrict the published host address.

Request admission is based on the external request authority, not the internal
listening port. Loopback authorities are accepted on arbitrary ports. Explicit
non-loopback browser origins may be listed in
`BEYOND_SLIDES_TRUSTED_ORIGINS`; an `Origin` header must match both the request
authority and an exact configured origin.

The multi-stage Docker build compiles the locked Rust source and installs the
pinned managed PDFium, FFmpeg, and `ffprobe` assets next to the executable. The
runtime stage does not install system FFmpeg or Poppler. Application data and
first-use model downloads live in `/data`.

## Consequences

- Arbitrary host-to-container port mappings work without weakening the default
  source-run bind address.
- The final image is larger because it contains verified native tools, but its
  PDF and media behavior does not depend on mutable system packages.
- Trusted origins protect the local process from ordinary hostile browser
  origins. They do not add users, authorization, tenant isolation, or TLS, so
  the service remains unsuitable for direct public exposure.
- Chromium and the Codex CLI remain optional and are not part of the core
  Docker image.
