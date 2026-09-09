//! Resolution and packaging of the optional portable Chromium runtime.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::runtime_tools::{self, RuntimeToolError};

pub const CHROMIUM_VERSION: &str = "152.0.7977.82-1";
const CHROMIUM_FILE_NAME: &str = "ungoogled-chromium.AppImage";
const EXTRACTED_CHROMIUM_FILE_NAME: &str = "chrome";
const CHROMIUM_URL: &str = "https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/releases/download/152.0.7977.82-1/ungoogled-chromium-152.0.7977.82-1-x86_64.AppImage";
const CHROMIUM_BYTES: u64 = 202_156_536;
const CHROMIUM_SHA256: &str = "b5915d2c380547719498a8186a63fc8dafad61806e5716bc39f12002753822ad";
const EXTRACTED_CHROMIUM_BYTES: u64 = 515_714_256;
const EXTRACTED_CHROMIUM_SHA256: &str =
    "35d642cd0f5a4c4ebe347d687910e0f2e3fd5889d368599045697140f45a3147";
const PORTABLE_LICENSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/ungoogled-chromium-portablelinux.LICENSE"
));
const CHROMIUM_LICENSE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/third_party/chromium.LICENSE"
));

/// One verified browser installed next to the BeyondSlides executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedChromium {
    executable: PathBuf,
    appimage: bool,
}

impl PackagedChromium {
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub const fn is_appimage(&self) -> bool {
        self.appimage
    }
}

/// Returns the browser from a packaged layout, if one is present.
///
/// Ordinary source installations continue to use chromiumoxide's system
/// browser discovery instead of downloading a browser implicitly.
pub fn packaged_chromium() -> Result<Option<PackagedChromium>, RuntimeToolError> {
    if !supports_portable_chromium() {
        return Ok(None);
    }
    let executable = std::env::current_exe().map_err(RuntimeToolError::CurrentExecutable)?;
    let Some(parent) = executable.parent() else {
        return Ok(None);
    };
    let chromium_directory = parent.join("chromium");
    let extracted = chromium_directory.join(EXTRACTED_CHROMIUM_FILE_NAME);
    if extracted.is_file() {
        runtime_tools::verify_file(
            &extracted,
            EXTRACTED_CHROMIUM_BYTES,
            EXTRACTED_CHROMIUM_SHA256,
        )?;
        return Ok(Some(PackagedChromium {
            executable: extracted,
            appimage: false,
        }));
    }
    let appimage = chromium_directory.join(CHROMIUM_FILE_NAME);
    if appimage.is_file() {
        runtime_tools::verify_file(&appimage, CHROMIUM_BYTES, CHROMIUM_SHA256)?;
        return Ok(Some(PackagedChromium {
            executable: appimage,
            appimage: true,
        }));
    }
    Ok(None)
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
