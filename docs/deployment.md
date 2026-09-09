# Deployment

BeyondSlides is a local single-user application. Its Docker image is useful for
reproducible installation and isolation, not for turning it into a public
multi-user service.

## Build and run

Build the release binary from the checked-in lockfile and prepare all pinned
native runtime tools in the image:

```sh
docker build --pull=false -t beyond-slides .
```

The final Debian runtime image installs neither FFmpeg nor Poppler. It contains
the BeyondSlides binary, executable-adjacent PDFium, FFmpeg, and `ffprobe`, their
upstream notices, CA certificates, and the C/C++ runtime libraries needed by the
Rust and PDFium binaries. Dense retrieval, OCR, and speech-recognition models
remain verified first-use downloads in the application data volume.

Run it with a persistent named volume. The host and container ports do not need
to match:

```sh
docker volume create beyond-slides-data
docker run --rm --name beyond-slides \
  -p 127.0.0.1:80:7842 \
  -v beyond-slides-data:/data \
  beyond-slides
```

Open `http://127.0.0.1/`. The image sets
`BEYOND_SLIDES_BIND_ADDRESS=0.0.0.0` inside the container so Docker can forward
traffic to it. Publishing to `127.0.0.1` keeps the application local to the
host; Docker translates port 80 to the application's port 7842 before the
request reaches BeyondSlides.

Do not omit the host address and publish the port to a shared network unless
you have added an authentication layer. BeyondSlides trusts loopback requests
but has no user accounts, authorization, tenant isolation, or TLS termination.

## Reverse proxies and trusted origins

The request guard accepts loopback hostnames on arbitrary ports. If a trusted
local reverse proxy exposes another origin, list the exact browser origin and
preserve its `Host` header:

```sh
docker run --rm --name beyond-slides \
  -p 127.0.0.1:8080:7842 \
  -e BEYOND_SLIDES_TRUSTED_ORIGINS=https://reader.example \
  -v beyond-slides-data:/data \
  beyond-slides
```

Multiple origins are comma-separated. Each value must contain only an `http`
or `https` scheme, hostname, and optional port. Browser requests with an
`Origin` header must match the request authority and one configured origin;
untrusted `Host` values are rejected. This protects the local process from
ordinary cross-origin browser requests, but it is not authentication.

For a non-container source run, `BEYOND_SLIDES_BIND_ADDRESS` defaults to
`127.0.0.1` and can be set to another IP explicitly.

## Reproducible smoke test

The verifier builds the image, maps an arbitrary host port, checks HTTP access,
confirms that no system FFmpeg or Poppler commands exist, imports a PDF plus a
timed transcript and recording, and decodes the test recording with the managed
FFmpeg binary:

```sh
BEYOND_SLIDES_DOCKER_SMOKE_PORT=18080 scripts/verify_docker.sh
```

To retest an existing image without rebuilding it:

```sh
BEYOND_SLIDES_SKIP_DOCKER_BUILD=1 scripts/verify_docker.sh beyond-slides
```

The verifier requires Docker, `curl`, and Python 3 on the host. The application
container itself does not require those commands.

## Optional integrations

The default Docker image deliberately omits Chromium and the optional Codex
CLI. Uploaded local sources and OpenAI-compatible model endpoints work normally.
Rain Classroom needs a compatible complete Chromium distribution, while Codex
mode needs an authenticated Codex CLI; neither optional integration blocks the
core Docker deployment.
