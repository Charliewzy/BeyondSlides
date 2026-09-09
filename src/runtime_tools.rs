//! Resolution and verified installation of native runtime tools.

use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

pub const RUNTIME_TOOLS_DIRECTORY_ENV: &str = "BEYOND_SLIDES_RUNTIME_TOOLS_DIR";
// This is the immutable ffmpeg-static release revision. Its executables report
// FFmpeg 7.0.2; the bundle revision is kept in paths so upstream replacement is
// always explicit.
const MEDIA_BUNDLE_REVISION: &str = "b6.1.1";
const PDFIUM_VERSION: &str = "7881";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaTool {
    Ffmpeg,
    Ffprobe,
}

impl MediaTool {
    const fn name(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.name(), std::env::consts::EXE_SUFFIX)
    }
}

#[derive(Debug, Clone, Copy)]
struct MediaAsset {
    url: &'static str,
    compressed_bytes: u64,
    compressed_sha256: &'static str,
    executable_bytes: u64,
    executable_sha256: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct PdfiumAsset {
    url: &'static str,
    compressed_bytes: u64,
    compressed_sha256: &'static str,
    library_bytes: u64,
    library_sha256: &'static str,
    archive_path: &'static str,
}

#[derive(Debug, Clone)]
struct DownloadAsset {
    url: String,
    bytes: u64,
    sha256: &'static str,
}

/// Returns the managed FFmpeg executable, downloading the pinned build once
/// when no packaged or cached copy is available.
pub fn ffmpeg_path() -> Result<PathBuf, RuntimeToolError> {
    resolve_media_tool(MediaTool::Ffmpeg)
}

/// Returns the managed ffprobe executable from the same pinned distribution.
pub fn ffprobe_path() -> Result<PathBuf, RuntimeToolError> {
    resolve_media_tool(MediaTool::Ffprobe)
}

/// Returns the managed PDFium dynamic library. The library version is pinned
/// to the ABI selected by this crate's `pdfium-render` feature.
pub fn pdfium_library_path() -> Result<PathBuf, RuntimeToolError> {
    let asset = pdfium_asset()?;
    let relative = PathBuf::from(format!("pdfium-{PDFIUM_VERSION}"))
        .join(runtime_target())
        .join(pdfium_library_name());
    for root in packaged_roots()? {
        let candidate = root.join(&relative);
        if candidate.is_file() {
            verify_file(&candidate, asset.library_bytes, asset.library_sha256)?;
            return Ok(candidate);
        }
    }

    let destination = cache_root()?.join(&relative);
    install_pdfium(asset, &destination)?;
    Ok(destination)
}

/// Installs every native runtime tool beneath `destination` using the same
/// versioned layout recognized by the packaged-tool resolver.
///
/// Release builders can call this once and place the resulting directory next
/// to the BeyondSlides executable. Normal source builds do not need to call it;
/// missing tools are downloaded into the user's cache on first use.
pub fn install_all(destination: &Path) -> Result<(), RuntimeToolError> {
    for tool in [MediaTool::Ffmpeg, MediaTool::Ffprobe] {
        let target = destination
            .join(format!("ffmpeg-{MEDIA_BUNDLE_REVISION}"))
            .join(runtime_target())
            .join(tool.file_name());
        install_media_asset(tool, media_asset(tool)?, &target)?;
    }
    let media_directory = destination
        .join(format!("ffmpeg-{MEDIA_BUNDLE_REVISION}"))
        .join(runtime_target());
    for (file_name, asset) in media_notice_assets()? {
        install_download_asset(&asset, &media_directory.join(file_name))?;
    }
    let target = destination
        .join(format!("pdfium-{PDFIUM_VERSION}"))
        .join(runtime_target())
        .join(pdfium_library_name());
    install_pdfium(pdfium_asset()?, &target)
}

fn resolve_media_tool(tool: MediaTool) -> Result<PathBuf, RuntimeToolError> {
    let asset = media_asset(tool)?;
    let relative = PathBuf::from(format!("ffmpeg-{MEDIA_BUNDLE_REVISION}"))
        .join(runtime_target())
        .join(tool.file_name());

    for root in packaged_roots()? {
        let candidate = root.join(&relative);
        if candidate.is_file() {
            verify_file(&candidate, asset.executable_bytes, asset.executable_sha256)?;
            return Ok(candidate);
        }
    }

    let cache_root = cache_root()?;
    let destination = cache_root.join(&relative);
    install_media_asset(tool, asset, &destination)?;
    Ok(destination)
}

fn packaged_roots() -> Result<Vec<PathBuf>, RuntimeToolError> {
    let mut roots = Vec::new();
    if let Some(configured) = std::env::var_os(RUNTIME_TOOLS_DIRECTORY_ENV) {
        roots.push(PathBuf::from(configured));
    }
    let executable = std::env::current_exe().map_err(RuntimeToolError::CurrentExecutable)?;
    if let Some(parent) = executable.parent() {
        roots.push(parent.join("runtime-tools"));
    }
    Ok(roots)
}

fn cache_root() -> Result<PathBuf, RuntimeToolError> {
    dirs::cache_dir()
        .map(|root| root.join("beyond-slides").join("runtime-tools"))
        .ok_or(RuntimeToolError::NoCacheDirectory)
}

fn install_media_asset(
    tool: MediaTool,
    asset: MediaAsset,
    destination: &Path,
) -> Result<(), RuntimeToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| RuntimeToolError::InvalidDestination(destination.to_owned()))?;
    fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(parent.join("download.lock"))
        .map_err(RuntimeToolError::Io)?;
    lock.lock().map_err(RuntimeToolError::Io)?;

    if destination.is_file()
        && verify_file(destination, asset.executable_bytes, asset.executable_sha256).is_ok()
    {
        return Ok(());
    }

    let temporary = tempfile::tempdir_in(parent).map_err(RuntimeToolError::Io)?;
    let archive = temporary.path().join(format!("{}.gz", tool.name()));
    download_verified(
        &archive,
        asset.url,
        asset.compressed_bytes,
        asset.compressed_sha256,
    )?;
    let staged = temporary.path().join(tool.file_name());
    let mut compressed = GzDecoder::new(File::open(&archive).map_err(RuntimeToolError::Io)?);
    let mut output = File::create(&staged).map_err(RuntimeToolError::Io)?;
    io::copy(&mut compressed, &mut output).map_err(RuntimeToolError::Io)?;
    output.flush().map_err(RuntimeToolError::Io)?;
    drop(output);
    verify_file(&staged, asset.executable_bytes, asset.executable_sha256)?;
    make_executable(&staged)?;
    replace_file(&staged, destination)?;
    Ok(())
}

fn install_pdfium(asset: PdfiumAsset, destination: &Path) -> Result<(), RuntimeToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| RuntimeToolError::InvalidDestination(destination.to_owned()))?;
    fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(parent.join("download.lock"))
        .map_err(RuntimeToolError::Io)?;
    lock.lock().map_err(RuntimeToolError::Io)?;
    let license_directory = parent.join("licenses");
    if destination.is_file()
        && license_directory.join("LICENSE").is_file()
        && verify_file(destination, asset.library_bytes, asset.library_sha256).is_ok()
    {
        return Ok(());
    }

    let temporary = tempfile::tempdir_in(parent).map_err(RuntimeToolError::Io)?;
    let archive_path = temporary.path().join("pdfium.tgz");
    download_verified(
        &archive_path,
        asset.url,
        asset.compressed_bytes,
        asset.compressed_sha256,
    )?;
    let staged = temporary.path().join(pdfium_library_name());
    let compressed = GzDecoder::new(File::open(&archive_path).map_err(RuntimeToolError::Io)?);
    let mut archive = tar::Archive::new(compressed);
    let mut found = false;
    for entry in archive.entries().map_err(RuntimeToolError::Io)? {
        let mut entry = entry.map_err(RuntimeToolError::Io)?;
        let entry_path = entry.path().map_err(RuntimeToolError::Io)?.into_owned();
        if entry_path == Path::new(asset.archive_path) {
            let mut output = File::create(&staged).map_err(RuntimeToolError::Io)?;
            io::copy(&mut entry, &mut output).map_err(RuntimeToolError::Io)?;
            output.flush().map_err(RuntimeToolError::Io)?;
            found = true;
        } else if entry.header().entry_type().is_file()
            && (entry_path == Path::new("LICENSE") || entry_path.starts_with(Path::new("licenses")))
        {
            let relative = entry_path.strip_prefix("licenses").unwrap_or(&entry_path);
            let target = license_directory.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
            }
            let mut output = File::create(target).map_err(RuntimeToolError::Io)?;
            io::copy(&mut entry, &mut output).map_err(RuntimeToolError::Io)?;
            output.flush().map_err(RuntimeToolError::Io)?;
        }
    }
    if !found {
        return Err(RuntimeToolError::MissingArchiveEntry {
            archive: archive_path,
            entry: asset.archive_path.into(),
        });
    }
    verify_file(&staged, asset.library_bytes, asset.library_sha256)?;
    replace_file(&staged, destination)
}

fn install_download_asset(
    asset: &DownloadAsset,
    destination: &Path,
) -> Result<(), RuntimeToolError> {
    let parent = destination
        .parent()
        .ok_or_else(|| RuntimeToolError::InvalidDestination(destination.to_owned()))?;
    fs::create_dir_all(parent).map_err(RuntimeToolError::Io)?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(parent.join("download.lock"))
        .map_err(RuntimeToolError::Io)?;
    lock.lock().map_err(RuntimeToolError::Io)?;
    if destination.is_file() && verify_file(destination, asset.bytes, asset.sha256).is_ok() {
        return Ok(());
    }
    let temporary = tempfile::tempdir_in(parent).map_err(RuntimeToolError::Io)?;
    let staged = temporary.path().join("download");
    download_verified(&staged, &asset.url, asset.bytes, asset.sha256)?;
    replace_file(&staged, destination)
}

fn download_verified(
    destination: &Path,
    url: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), RuntimeToolError> {
    let destination = destination.to_owned();
    let url = url.to_owned();
    let expected_sha256 = expected_sha256.to_owned();
    std::thread::spawn(move || {
        download_verified_on_blocking_thread(&destination, &url, expected_bytes, &expected_sha256)
    })
    .join()
    .map_err(|_| RuntimeToolError::DownloadWorkerPanicked)?
}

fn download_verified_on_blocking_thread(
    destination: &Path,
    url: &str,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), RuntimeToolError> {
    let mut response = reqwest::blocking::Client::builder()
        .user_agent(concat!("BeyondSlides/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(30 * 60))
        .build()
        .map_err(RuntimeToolError::Download)?
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(RuntimeToolError::Download)?;
    let mut output = File::create(destination).map_err(RuntimeToolError::Io)?;
    io::copy(&mut response, &mut output).map_err(RuntimeToolError::Io)?;
    output.flush().map_err(RuntimeToolError::Io)?;
    verify_file(destination, expected_bytes, expected_sha256)
}

fn verify_file(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
) -> Result<(), RuntimeToolError> {
    let metadata = fs::metadata(path).map_err(RuntimeToolError::Io)?;
    if metadata.len() != expected_bytes {
        return Err(RuntimeToolError::Integrity {
            path: path.to_owned(),
            expected: expected_sha256.into(),
            actual: format!("{} bytes", metadata.len()),
        });
    }
    let mut input = File::open(path).map_err(RuntimeToolError::Io)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(RuntimeToolError::Io)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if actual != expected_sha256 {
        return Err(RuntimeToolError::Integrity {
            path: path.to_owned(),
            expected: expected_sha256.into(),
            actual,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), RuntimeToolError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(RuntimeToolError::Io)?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(RuntimeToolError::Io)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), RuntimeToolError> {
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), RuntimeToolError> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination).map_err(RuntimeToolError::Io)?;
    }
    fs::rename(source, destination).map_err(RuntimeToolError::Io)
}

const fn runtime_target() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-x64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "win32-x64"
    } else {
        "unsupported"
    }
}

fn media_asset(tool: MediaTool) -> Result<MediaAsset, RuntimeToolError> {
    let target = runtime_target();
    let asset = match (target, tool) {
        ("linux-x64", MediaTool::Ffmpeg) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-linux-x64.gz",
            compressed_bytes: 29_354_986,
            compressed_sha256: "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa",
            executable_bytes: 79_826_272,
            executable_sha256: "e7e7fb30477f717e6f55f9180a70386c62677ef8a4d4d1a5d948f4098aa3eb99",
        },
        ("linux-x64", MediaTool::Ffprobe) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-linux-x64.gz",
            compressed_bytes: 29_276_839,
            compressed_sha256: "25d9b6ccb05e3d9de9e04e31e2506d8dd7f9f0418981965ac6df12e8d3afd067",
            executable_bytes: 79_665_792,
            executable_sha256: "4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d",
        },
        ("linux-arm64", MediaTool::Ffmpeg) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-linux-arm64.gz",
            compressed_bytes: 25_568_691,
            compressed_sha256: "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0",
            executable_bytes: 51_134_160,
            executable_sha256: "6bb182d0d75d23028db82e9e4f723ca69b853d055698486e6984ddb2c06fb8ce",
        },
        ("linux-arm64", MediaTool::Ffprobe) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-linux-arm64.gz",
            compressed_bytes: 25_493_573,
            compressed_sha256: "2ab6aba60ee84412dff9188720703376cb4e7aaf7e0b5e43aa8249f2acae5bf8",
            executable_bytes: 50_994_160,
            executable_sha256: "d17ae9b4c297d48e2521ba14e417bb0537c6ff77c584cdbcd6bb0d8d0307a2e8",
        },
        ("darwin-x64", MediaTool::Ffmpeg) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-darwin-x64.gz",
            compressed_bytes: 25_296_431,
            compressed_sha256: "929b375c1182d956c51f7ac25e0b2b0411fb01f6f407aa15c9758efeb4242106",
            executable_bytes: 78_862_176,
            executable_sha256: "ebdddc936f61e14049a2d4b549a412b8a40deeff6540e58a9f2a2da9e6b18894",
        },
        ("darwin-x64", MediaTool::Ffprobe) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-darwin-x64.gz",
            compressed_bytes: 25_239_438,
            compressed_sha256: "d4da574d6e2e197bd259b47d69cf262df9e312af24ad960444f6d806d3d4c186",
            executable_bytes: 78_780_408,
            executable_sha256: "fa3add0ce901f7241abe0dfc0155d958fc834aca3f8ce61f87cc712ae669c1e0",
        },
        ("darwin-arm64", MediaTool::Ffmpeg) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-darwin-arm64.gz",
            compressed_bytes: 19_246_198,
            compressed_sha256: "8923876afa8db5585022d7860ec7e589af192f441c56793971276d450ed3bbfa",
            executable_bytes: 45_568_216,
            executable_sha256: "a90e3db6a3fd35f6074b013f948b1aa45b31c6375489d39e572bea3f18336584",
        },
        ("darwin-arm64", MediaTool::Ffprobe) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-darwin-arm64.gz",
            compressed_bytes: 19_207_077,
            compressed_sha256: "d986a8ec7b030899fe66a8a288ed809a3543338705a3ce178cfb85869c5d80be",
            executable_bytes: 45_528_808,
            executable_sha256: "bb2db6f5d8cef919da12fbf592119a987202a8c060a886f3cab091f9cab90b64",
        },
        ("win32-x64", MediaTool::Ffmpeg) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffmpeg-win32-x64.gz",
            compressed_bytes: 29_581_307,
            compressed_sha256: "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77",
            executable_bytes: 82_797_568,
            executable_sha256: "04e1307997530f9cf2fe35cba2ca7e8875ca91da02f89d6c7243df819c94ad00",
        },
        ("win32-x64", MediaTool::Ffprobe) => MediaAsset {
            url: "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.1.1/ffprobe-win32-x64.gz",
            compressed_bytes: 29_521_644,
            compressed_sha256: "f309e6223ad89d2fe54bccd420a7709b66fd27540674e92309578ed491a43c8d",
            executable_bytes: 82_668_032,
            executable_sha256: "3a7e2dc003dc2cd1472827e4c7c4f056ae1ae0ae7c5bbc580c99b49827351ba4",
        },
        _ => {
            return Err(RuntimeToolError::UnsupportedPlatform {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
            });
        }
    };
    Ok(asset)
}

fn media_notice_assets() -> Result<[(&'static str, DownloadAsset); 2], RuntimeToolError> {
    let (license_bytes, license_sha256, readme_bytes, readme_sha256) = match runtime_target() {
        "linux-x64" => (
            35_147,
            "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
            2_235,
            "72f4b1b06d419d22ace6e7cc75f06826f90737345aa0b1736158929f4aacc537",
        ),
        "linux-arm64" => (
            35_147,
            "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
            2_217,
            "d6777d2fd276b23f0ac6666fa619e88ffe4826521881c7ff83836e30cb4acec2",
        ),
        "darwin-x64" => (
            4_346,
            "2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af",
            6_227,
            "e88a0325f8e5b75210355e37341824f074d3cd82def2125be54c914b62848a36",
        ),
        "darwin-arm64" => (
            4_376,
            "cb48bf09a11f5fb576cddb0431c8f5ed0a60157a9ec942adffc13907cbe083f2",
            1_810,
            "05ba4b92c96605434b1aaae3eedf5a2c280c9607bf78ffca9a5b536d9af2dc6a",
        ),
        "win32-x64" => (
            35_147,
            "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
            39_494,
            "a636a7183c58006351acbaf35303c0ed85c6e1320fd4e80de453ba6157de6311",
        ),
        _ => {
            return Err(RuntimeToolError::UnsupportedPlatform {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
            });
        }
    };
    let prefix = format!(
        "https://github.com/eugeneware/ffmpeg-static/releases/download/{MEDIA_BUNDLE_REVISION}/{}",
        runtime_target()
    );
    Ok([
        (
            "LICENSE.ffmpeg.txt",
            DownloadAsset {
                url: format!("{prefix}.LICENSE"),
                bytes: license_bytes,
                sha256: license_sha256,
            },
        ),
        (
            "README.ffmpeg.txt",
            DownloadAsset {
                url: format!("{prefix}.README"),
                bytes: readme_bytes,
                sha256: readme_sha256,
            },
        ),
    ])
}

const fn pdfium_library_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

fn pdfium_asset() -> Result<PdfiumAsset, RuntimeToolError> {
    let asset = match runtime_target() {
        "linux-x64" => PdfiumAsset {
            url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-x64.tgz",
            compressed_bytes: 3_644_759,
            compressed_sha256: "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
            library_bytes: 7_645_184,
            library_sha256: "f728930966f503652b92acc89b9374a2eeca00ce42e26dccd3e4b5c5161b2d64",
            archive_path: "lib/libpdfium.so",
        },
        "linux-arm64" => PdfiumAsset {
            url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-linux-arm64.tgz",
            compressed_bytes: 3_588_127,
            compressed_sha256: "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674",
            library_bytes: 7_834_256,
            library_sha256: "6252fce3da45e7f0dc5b27f4d4e1a1456ca3f7734cdb04f927967df772127478",
            archive_path: "lib/libpdfium.so",
        },
        "darwin-x64" => PdfiumAsset {
            url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-mac-x64.tgz",
            compressed_bytes: 3_588_813,
            compressed_sha256: "6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b",
            library_bytes: 7_471_800,
            library_sha256: "4eaad6c3e8d786cf6f66a45d7d014edf5c65f372f98c3070e66595ebb50e43d9",
            archive_path: "lib/libpdfium.dylib",
        },
        "darwin-arm64" => PdfiumAsset {
            url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-mac-arm64.tgz",
            compressed_bytes: 3_533_019,
            compressed_sha256: "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40",
            library_bytes: 7_732_336,
            library_sha256: "1bc45b15466b34cef96641ce25c77a876e70010c6b114f909dda2f5325fc5bd7",
            archive_path: "lib/libpdfium.dylib",
        },
        "win32-x64" => PdfiumAsset {
            url: "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz",
            compressed_bytes: 3_733_154,
            compressed_sha256: "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
            library_bytes: 7_211_520,
            library_sha256: "79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118",
            archive_path: "bin/pdfium.dll",
        },
        _ => {
            return Err(RuntimeToolError::UnsupportedPlatform {
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
            });
        }
    };
    Ok(asset)
}

#[derive(Debug)]
pub enum RuntimeToolError {
    UnsupportedPlatform {
        os: String,
        arch: String,
    },
    CurrentExecutable(io::Error),
    NoCacheDirectory,
    InvalidDestination(PathBuf),
    MissingArchiveEntry {
        archive: PathBuf,
        entry: String,
    },
    DownloadWorkerPanicked,
    Download(reqwest::Error),
    Io(io::Error),
    Integrity {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for RuntimeToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform { os, arch } => {
                write!(
                    formatter,
                    "managed runtime tools do not support {os}/{arch}"
                )
            }
            Self::CurrentExecutable(error) => {
                write!(
                    formatter,
                    "could not locate the BeyondSlides executable: {error}"
                )
            }
            Self::NoCacheDirectory => {
                formatter.write_str("could not locate a runtime cache directory")
            }
            Self::InvalidDestination(path) => {
                write!(
                    formatter,
                    "runtime tool path {} has no parent",
                    path.display()
                )
            }
            Self::MissingArchiveEntry { archive, entry } => write!(
                formatter,
                "runtime archive {} does not contain {entry}",
                archive.display()
            ),
            Self::DownloadWorkerPanicked => {
                formatter.write_str("runtime-tool download worker panicked")
            }
            Self::Download(error) => {
                write!(formatter, "could not download a runtime tool: {error}")
            }
            Self::Io(error) => write!(formatter, "could not install a runtime tool: {error}"),
            Self::Integrity {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "runtime tool {} failed integrity validation: expected {expected}, found {actual}",
                path.display()
            ),
        }
    }
}

impl Error for RuntimeToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) | Self::Io(error) => Some(error),
            Self::Download(error) => Some(error),
            Self::UnsupportedPlatform { .. }
            | Self::NoCacheDirectory
            | Self::InvalidDestination(_)
            | Self::MissingArchiveEntry { .. }
            | Self::DownloadWorkerPanicked
            | Self::Integrity { .. } => None,
        }
    }
}
