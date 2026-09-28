# Linux standalone package

The current standalone preview targets a contemporary x86-64 Linux desktop
with glibc and a graphical session. It packages the BeyondSlides release binary,
the managed PDFium/FFmpeg/ffprobe layout, and a complete pinned
ungoogled-Chromium AppImage. It does not package OCR, ASR, or embedding model
weights, which retain the application's verified first-use download behavior.
The optional Codex backend still requires an installed, authenticated Codex
CLI; OpenAI-compatible endpoints need no additional local program.

For Ubuntu 24.04 and later, prefer the `.deb` package below. Ubuntu's AppArmor
policy can deny the user namespace needed by Chromium inside a relocatable
AppImage. The installed package gives Chromium a stable executable path and
grants that path only the namespace permission needed by Chromium's own
sandbox. It does not use `--no-sandbox` or relax the global host policy.

## Ubuntu package

Build the package from an x86-64 Linux checkout with `dpkg-deb` installed:

```sh
scripts/package_deb_linux.sh
```

The result is `dist/beyond-slides_0.1.0-1_amd64.deb`. Install or update it with:

```sh
sudo apt install ./dist/beyond-slides_0.1.0-1_amd64.deb
```

Launch **BeyondSlides** from the desktop application menu, or run
`/opt/beyond-slides/BeyondSlides`. Lecture data remains under
`${XDG_DATA_HOME:-~/.local/share}/beyond-slides` and survives package upgrades
and removal. `sudo apt remove beyond-slides` removes the program;
`sudo apt purge beyond-slides` also removes its local AppArmor override file,
but never deletes user lecture data.

The current package is approximately 222 MiB to download and 967 MiB installed.
Unlike the portable archive, it expands Chromium under `/opt` so Ubuntu can
apply a fixed, path-scoped AppArmor profile. It targets Ubuntu 24.04 or later
on amd64 and declares the browser's ordinary desktop-library dependencies.

Verify a built package without installing it on the host:

```sh
scripts/verify_deb_linux.sh dist/beyond-slides_0.1.0-1_amd64.deb
```

The verifier checks ownership, metadata, the parsed AppArmor profile, browser,
managed runtimes, launcher lifecycle, and Rain Classroom browser integration.
The package has also been installed and exercised in a clean Ubuntu 24.04
container. A machine where AppArmor enforcement is active is still required to
confirm the original Ubuntu desktop sandbox failure is resolved.

## Build

From the repository root:

```sh
scripts/package_standalone_linux.sh
```

The builder compiles `Cargo.lock` in release mode, strips a copied executable,
installs the same verified native assets used by source and Docker runs, obtains
the pinned browser, retains third-party notices, and publishes only after every
component validates. It refuses to replace an existing output. The results are:

```text
dist/beyond-slides-linux-x86_64/
dist/beyond-slides-linux-x86_64.tar.gz
```

For an already downloaded matching browser, release builders may avoid another
transfer while retaining normal size and SHA-256 validation:

```sh
BEYOND_SLIDES_CHROMIUM_APPIMAGE=/path/to/ungoogled-chromium.AppImage \
  scripts/package_standalone_linux.sh
```

The pinned browser is ungoogled-Chromium `152.0.7977.82-1` from the project's
portable-Linux release. The AppImage is 202,156,536 bytes with SHA-256
`b5915d2c380547719498a8186a63fc8dafad61806e5716bc39f12002753822ad`.
The tested complete directory is approximately 424 MiB and its gzip-compressed
download is approximately 283 MiB. These measurements exclude user data and
first-use ML models.

## Run and update

Extract the complete archive and run `BeyondSlides` from inside the resulting
directory. The launcher:

1. sets the executable-adjacent managed-runtime path;
2. starts the controller on `127.0.0.1:7842`;
3. opens the bundled browser at that origin in application mode; and
4. sends a graceful shutdown signal to the controller after the browser exits.

If direct AppImage mounting is unavailable, the launcher retries Chromium's
slower extract-and-run mode. Set `BEYOND_SLIDES_PORT` when port 7842 is occupied,
or `BEYOND_SLIDES_DATA_DIR` to choose the persistent-data location. The default
is `${XDG_DATA_HOME:-~/.local/share}/beyond-slides`.

Program files and user data are intentionally separate. Updating is therefore
whole-bundle replacement: close BeyondSlides, extract the new program directory,
and replace the old program directory. Lecture sources, checkpoints, model
caches, reports, and the Rain Classroom profile remain untouched. There is no
automatic updater or rollback protocol yet.

## Verify

After packaging:

```sh
scripts/verify_standalone_linux.sh dist/beyond-slides-linux-x86_64
```

The verifier substitutes a test browser to prove that the launcher starts and
stops its controller, checks the bundled Chromium and native tools, then starts
the real packaged browser through the Rain Classroom adapter and captures its
login view. The test does not require scanning the QR code or contacting an
analysis model.

The portable browser avoids a separate Chrome installation but cannot remove
the ordinary baseline requirements of a Linux graphical application: a
supported kernel, glibc, display server, and working desktop graphics stack.
On Ubuntu 24.04 or later, use the `.deb` package if the portable AppImage reports
`No usable sandbox`. The package is not a macOS or Windows build.
