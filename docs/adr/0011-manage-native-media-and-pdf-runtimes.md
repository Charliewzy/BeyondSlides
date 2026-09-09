# Manage native media and PDF runtimes

BeyondSlides must run from a source checkout, container, or standalone bundle
without requiring users to install FFmpeg or Poppler. Resolve pinned native
assets from an explicitly configured or executable-adjacent `runtime-tools`
directory first, then the user cache, and otherwise download them with exact
size and SHA-256 verification. All media conversion and probing uses the managed
FFmpeg/ffprobe build; PDF text extraction and rendering uses one process-wide,
thread-safe PDFium 7881 binding. Do not fall back to programs found on `PATH`,
because that would make behavior and supported formats depend on the host.

Release preparation can preinstall the same versioned layout and includes the
upstream notices. Ordinary source builds may instead incur a network download
on first use. The selected Linux and Windows FFmpeg builds are GPLv3 programs
distributed separately from the MIT-licensed BeyondSlides executable; bundles
must retain their supplied license and build information.
