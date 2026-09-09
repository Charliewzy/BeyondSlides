# ADR 0013: Package a portable Linux browser

## Status

Accepted

## Context

Rain Classroom automation requires a complete Chromium-family browser, and a
one-click local application also needs a browser surface for the existing web
UI. Relying on host browser discovery would leave the standalone package with a
large undocumented system dependency. Expanding a raw Chromium snapshot would
make the installed program substantially larger and would still require a
desktop-specific dependency set.

User data must survive whole-program replacement, while Codex and several ML
models are deliberately optional or first-use resources.

## Decision

The Linux x86-64 standalone package contains a size- and SHA-256-verified
ungoogled-Chromium AppImage pinned to version `152.0.7977.82-1`. The compressed
AppImage remains intact rather than being expanded into the program directory.
Its Chromium and portable-build licenses, immutable download URL, version, and
digest accompany it.

An executable shell launcher starts the existing loopback controller, opens the
packaged browser in application mode, and gracefully stops the controller when
the browser exits. It attempts normal AppImage mounting first and falls back to
extract-and-run mode. Rain Classroom prefers the same adjacent verified browser
and uses extract-and-run mode directly; ordinary source builds retain system
browser discovery instead of silently downloading Chromium.

Persistent application data uses the user's data directory, not the program
directory. ML model weights retain verified first-use downloads, and the Codex
CLI remains an optional external integration.

## Consequences

- Linux users can run the complete core application and Rain Classroom adapter
  without installing Chromium, FFmpeg, Poppler, Python, or PyTorch.
- The tested package is about 424 MiB unpacked and 283 MiB as a `.tar.gz`, before
  user data and downloaded models.
- FUSE-less machines remain supported, but their browser startup is slower due
  to temporary extraction.
- Updates initially replace the whole program directory; persistent lectures
  and checkpoints remain in place, but there is no automatic updater.
- The package still assumes a modern glibc Linux graphical environment. Windows
  and macOS require their own browser assets, launchers, and native verification.
