BeyondSlides standalone preview for Linux x86-64
================================================

Run the executable file named "BeyondSlides". It starts the local application
server, opens the bundled Chromium in application mode, and stops the controller
when that browser window closes.

Lecture data, checkpoints, downloaded ML models, and Rain Classroom login state
are stored under ~/.local/share/beyond-slides by default. Replacing this program
folder with a newer bundle does not remove that data. Set
BEYOND_SLIDES_DATA_DIR before launching to choose another location.

The application listens only on 127.0.0.1:7842. Set BEYOND_SLIDES_PORT if that
port is already occupied. Controller diagnostics are written to controller.log
inside the data directory; processing logs remain available in the application.

The bundle includes separately distributed, pinned builds of ungoogled-Chromium,
FFmpeg, ffprobe, and PDFium with their notices. OCR, speech-recognition, and
embedding models download with integrity verification when first needed. Codex
mode remains optional and requires an installed, authenticated Codex CLI.

Target: a contemporary x86-64 Linux desktop with glibc and a graphical session.
The Chromium AppImage normally uses FUSE and automatically falls back to its
slower extract-and-run mode when direct mounting is unavailable.

Ubuntu 24.04 and later may block this portable browser through its AppArmor
user-namespace policy. Use the project's Ubuntu .deb package on those systems;
it installs the same verified browser at a stable path and applies a narrowly
scoped AppArmor profile without disabling Chromium's sandbox.
