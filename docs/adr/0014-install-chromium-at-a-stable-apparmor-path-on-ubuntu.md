# ADR 0014: Install Chromium at a stable AppArmor path on Ubuntu

## Status

Accepted

## Context

Ubuntu 24.04 and later restrict unprivileged user namespaces through AppArmor.
Chromium needs a user namespace for its renderer sandbox, but the portable
AppImage from ADR 0013 runs its inner browser from a transient mount or
extraction path. A narrow host policy cannot reliably name that executable.
Disabling the global restriction or passing `--no-sandbox` would weaken the
host or browser security model for the sake of one application.

## Decision

The Ubuntu amd64 release is also distributed as a `.deb`. It expands the same
pinned, verified Chromium build into `/opt/beyond-slides/chromium/chrome` and
installs an AppArmor 4 profile for exactly that path. The profile remains
unconfined except for explicitly granting `userns`; Chromium's own sandbox
remains enabled. Package installation reloads the profile when AppArmor is
active, and purge removes the package-specific local policy artifacts.

The package targets Ubuntu 24.04 or later, declares AppArmor and Chromium's
desktop library dependencies, and retains all managed PDFium and media tools.
The relocatable AppImage archive remains available for Linux environments where
its sandbox can already create a user namespace.

## Consequences

- Ubuntu users get a normal application-menu entry, package-managed updates,
  and a stable browser path compatible with the host AppArmor policy.
- The package does not disable the global user-namespace restriction and does
  not launch Chromium with `--no-sandbox`.
- Expanding Chromium increases the installed footprint to about 949 MiB, while
  xz compression keeps the current `.deb` near 211 MiB.
- The package is deliberately less portable than the archive: it targets amd64
  Ubuntu 24.04+ and requires administrator access to install.
- Container tests can verify package installation and browser startup, but the
  original failure must finally be checked on an AppArmor-enforcing desktop.
