//! Resolution and packaging of the optional portable Chromium runtime.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::runtime_tools::{self, RuntimeToolError};

pub const CHROMIUM_VERSION: &str = "152.0.7977.82-1";
const CHROMIUM_FILE_NAME: &str = "ungoogled-chromium.AppImage";
const CHROMIUM_URL: &str = "https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/releases/download/152.0.7977.82-1/ungoogled-chromium-152.0.7977.82-1-x86_64.AppImage";
const CHROMIUM_BYTES: u64 = 202_156_536;
const CHROMIUM_SHA256: &str = "b5915d2c380547719498a8186a63fc8dafad61806e5716bc39f12002753822ad";
const PORTABLE_LICENSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/ungoogled-chromium-portablelinux.LICENSE"
));
const CHROMIUM_LICENSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/chromium.LICENSE"
));

/// Returns the executable from a standalone layout, if one is present.
///
/// Ordinary source installations continue to use chromiumoxide's system
/// browser discovery instead of downloading a browser implicitly.
pub fn packaged_chromium_path() -> Result<Option<PathBuf>, RuntimeToolError> {
    if !supports_portable_chromium() {
        return Ok(None);
    }
    let executable = std::env::current_exe().map_err(RuntimeToolError::CurrentExecutable)?;
    let Some(parent) = executable.parent() else {
        return Ok(None);
    };
    let candidate = parent.join("chromium").join(CHROMIUM_FILE_NAME);
    if !candidate.is_file() {
        return Ok(None);
    }
    runtime_tools::verify_file(&candidate, CHROMIUM_BYTES, CHROMIUM_SHA256)?;
    Ok(Some(candidate))
}

/// Downloads and verifies the complete portable browser used by Linux
/// standalone packages.
pub fn install(destination: &Path) -> Result<PathBuf, RuntimeToolError> {
    ensure_supported()?;
    fs::create_dir_all(destination).map_err(RuntimeToolError::Io)?;
    let executable = destination.join(CHROMIUM_FILE_NAME);
    runtime_tools::install_verified_executable(
        &executable,
        CHROMIUM_URL,
        CHROMIUM_BYTES,
        CHROMIUM_SHA256,
    )?;
    fs::write(
        destination.join("README.chromium.txt"),
        format!(
            "BeyondSlides portable browser\n\n\
             ungoogled-chromium {CHROMIUM_VERSION}\n\
             {CHROMIUM_URL}\n\
             SHA-256: {CHROMIUM_SHA256}\n\n\
             This separately distributed browser is used for the application\n\
             window and the optional Rain Classroom integration. Run\n\
             chrome://credits in the browser for bundled third-party notices.\n"
        ),
    )
    .map_err(RuntimeToolError::Io)?;
    fs::write(
        destination.join("LICENSE.ungoogled-chromium-portablelinux.txt"),
        PORTABLE_LICENSE,
    )
    .map_err(RuntimeToolError::Io)?;
    fs::write(destination.join("LICENSE.chromium.txt"), CHROMIUM_LICENSE)
        .map_err(RuntimeToolError::Io)?;
    Ok(executable)
}

const fn supports_portable_chromium() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
}

fn ensure_supported() -> Result<(), RuntimeToolError> {
    if supports_portable_chromium() {
        Ok(())
    } else {
        Err(RuntimeToolError::UnsupportedPlatform {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        })
    }
}
