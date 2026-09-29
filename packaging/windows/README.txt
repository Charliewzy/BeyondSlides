BeyondSlides for Windows x64
============================

Double-click BeyondSlides.exe. It starts the local BeyondSlides controller,
opens the bundled Chromium application window, and stops the controller when
that window closes. Processing workers continue independently so closing the
window does not discard completed checkpoints.

Application data, imported lectures, checkpoints, reports, browser profiles,
and controller diagnostics are stored under:

    %LOCALAPPDATA%\BeyondSlides

The application listens only on a free 127.0.0.1 port. Set
BEYOND_SLIDES_PORT to require a particular port, or BEYOND_SLIDES_DATA_DIR to
choose another data directory.

The package includes verified FFmpeg, ffprobe, PDFium, and Chromium builds.
ASR, OCR, and embedding model weights are downloaded and cached on first use.
An OpenAI-compatible endpoint needs no additional software. Codex mode remains
optional and requires a separately installed and authenticated Codex CLI.

This first public Windows build is unsigned. Windows SmartScreen may require
selecting "More info" and then "Run anyway". Verify downloads against the
SHA256SUMS.txt published with the package.
