//! Resolution and packaging of the optional portable Chromium runtime.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(target_os = "windows")]
use std::{
    fs::File,
    io::{self, Write},
};

use crate::runtime_tools::{self, RuntimeToolError};

pub const CHROMIUM_VERSION: &str = "152.0.7977.82";
#[cfg(target_os = "linux")]
const CHROMIUM_FILE_NAME: &str = "ungoogled-chromium.AppImage";
#[cfg(target_os = "linux")]
const EXTRACTED_CHROMIUM_FILE_NAME: &str = "chrome";
#[cfg(target_os = "linux")]
const CHROMIUM_URL: &str = "https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/releases/download/152.0.7977.82-1/ungoogled-chromium-152.0.7977.82-1-x86_64.AppImage";
#[cfg(target_os = "linux")]
const CHROMIUM_BYTES: u64 = 202_156_536;
#[cfg(target_os = "linux")]
const CHROMIUM_SHA256: &str = "b5915d2c380547719498a8186a63fc8dafad61806e5716bc39f12002753822ad";
#[cfg(target_os = "linux")]
const EXTRACTED_CHROMIUM_BYTES: u64 = 515_714_256;
#[cfg(target_os = "linux")]
const EXTRACTED_CHROMIUM_SHA256: &str =
    "35d642cd0f5a4c4ebe347d687910e0f2e3fd5889d368599045697140f45a3147";
#[cfg(target_os = "windows")]
const WINDOWS_ARCHIVE_ROOT: &str = "ungoogled-chromium_152.0.7977.82-1.1_windows_x64";
#[cfg(target_os = "windows")]
const WINDOWS_CHROMIUM_URL: &str = "https://github.com/ungoogled-software/ungoogled-chromium-windows/releases/download/152.0.7977.82-1.1/ungoogled-chromium_152.0.7977.82-1.1_windows_x64.zip";
#[cfg(target_os = "windows")]
const WINDOWS_CHROMIUM_ARCHIVE_BYTES: u64 = 197_181_183;
#[cfg(target_os = "windows")]
const WINDOWS_CHROMIUM_ARCHIVE_SHA256: &str =
    "0e49aaba44345da5110e3b4c5a7e5490334181a8aaa1ef25559f12d5be0acc53";
#[cfg(target_os = "windows")]
const WINDOWS_CHROMIUM_BYTES: u64 = 4_274_176;
#[cfg(target_os = "windows")]
const WINDOWS_CHROMIUM_SHA256: &str =
    "11ca7ce7021bdffcf102797f8a00c713f5295b5e7313f3f136efcd998422e6f2";
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
    #[cfg(target_os = "windows")]
    {
        let executable = chromium_directory.join("chrome.exe");
        if executable.is_file() {
            runtime_tools::verify_file(
                &executable,
                WINDOWS_CHROMIUM_BYTES,
                WINDOWS_CHROMIUM_SHA256,
            )?;
            return Ok(Some(PackagedChromium {
                executable,
                appimage: false,
            }));
        }
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
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
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Ok(None)
}

/// Downloads and verifies the complete portable browser used by Linux
/// standalone packages.
pub fn install(destination: &Path) -> Result<PathBuf, RuntimeToolError> {
    ensure_supported()?;
    #[cfg(target_os = "windows")]
    return install_windows(destination);
    #[cfg(target_os = "linux")]
    {
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
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    unreachable!("ensure_supported rejects this platform")
}

const fn supports_portable_chromium() -> bool {
    cfg!(all(
        any(target_os = "linux", target_os = "windows"),
        target_arch = "x86_64"
    ))
}

#[cfg(target_os = "windows")]
fn install_windows(destination: &Path) -> Result<PathBuf, RuntimeToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| RuntimeToolError::InvalidDestination(destination.to_owned()))?;
    fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
    let temporary = tempfile::tempdir_in(parent).map_err(RuntimeToolError::Io)?;
    let archive_path = temporary.path().join("chromium.zip");
    runtime_tools::download_verified(
        &archive_path,
        WINDOWS_CHROMIUM_URL,
        WINDOWS_CHROMIUM_ARCHIVE_BYTES,
        WINDOWS_CHROMIUM_ARCHIVE_SHA256,
    )?;

    let staged = temporary.path().join("chromium");
    fs::create_dir_all(&staged).map_err(RuntimeToolError::Io)?;
    let mut archive =
        zip::ZipArchive::new(File::open(&archive_path).map_err(RuntimeToolError::Io)?)
            .map_err(invalid_archive)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid_archive)?;
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            RuntimeToolError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsafe Chromium archive path {}", entry.name()),
            ))
        })?;
        let relative = enclosed.strip_prefix(WINDOWS_ARCHIVE_ROOT).map_err(|_| {
            RuntimeToolError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Chromium archive entry {} is outside {WINDOWS_ARCHIVE_ROOT}",
                    entry.name()
                ),
            ))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let output_path = staged.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(RuntimeToolError::Io)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
        }
        let mut output = File::create(&output_path).map_err(RuntimeToolError::Io)?;
        io::copy(&mut entry, &mut output).map_err(RuntimeToolError::Io)?;
        output.flush().map_err(RuntimeToolError::Io)?;
    }
    let executable = staged.join("chrome.exe");
    runtime_tools::verify_file(&executable, WINDOWS_CHROMIUM_BYTES, WINDOWS_CHROMIUM_SHA256)?;
    write_notices(
        &staged,
        "ungoogled-chromium Windows x64 portable ZIP",
        WINDOWS_CHROMIUM_URL,
        WINDOWS_CHROMIUM_ARCHIVE_SHA256,
    )?;
    if destination.exists() {
        fs::remove_dir_all(destination).map_err(RuntimeToolError::Io)?;
    }
    fs::rename(&staged, destination).map_err(RuntimeToolError::Io)?;
    Ok(destination.join("chrome.exe"))
}

#[cfg(target_os = "windows")]
fn invalid_archive(error: zip::result::ZipError) -> RuntimeToolError {
    RuntimeToolError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(target_os = "windows")]
fn write_notices(
    destination: &Path,
    distribution: &str,
    url: &str,
    sha256: &str,
) -> Result<(), RuntimeToolError> {
    fs::write(
        destination.join("README.chromium.txt"),
        format!(
            "BeyondSlides portable browser\n\n\
             {distribution}\n\
             Chromium {CHROMIUM_VERSION}\n\
             {url}\n\
             SHA-256: {sha256}\n\n\
             This separately distributed browser is used for the application\n\
             window and the optional Rain Classroom integration. Open\n\
             chrome://credits in the browser for bundled third-party notices.\n"
        ),
    )
    .map_err(RuntimeToolError::Io)?;
    fs::write(
        destination.join("LICENSE.ungoogled-chromium.txt"),
        PORTABLE_LICENSE,
    )
    .map_err(RuntimeToolError::Io)?;
    fs::write(destination.join("LICENSE.chromium.txt"), CHROMIUM_LICENSE)
        .map_err(RuntimeToolError::Io)
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
