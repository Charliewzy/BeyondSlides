//! Produces optional visual reference evidence for semantic-alignment evaluation.

use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use image::GrayImage;
use image_compare::Algorithm;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::SlideId;

const FRAME_WIDTH: u32 = 320;
const FRAME_HEIGHT: u32 = 180;
const FRAME_BYTES: usize = FRAME_WIDTH as usize * FRAME_HEIGHT as usize;
const SAMPLE_PERIOD_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct VisualAlignment {
    pub frame_width: u32,
    pub frame_height: u32,
    pub sample_period_ms: u64,
    pub frame_matches: Vec<FrameSlideMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct FrameSlideMatch {
    pub timestamp_ms: u64,
    pub best: SlideSimilarity,
    pub runner_up: Option<SlideSimilarity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct SlideSimilarity {
    pub slide_id: SlideId,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualAlignmentProgress {
    pub processed_ms: u64,
    pub total_ms: u64,
}

/// Samples a video once per second and compares each frame with every PDF page.
pub fn align_video_to_slides(
    video_path: &Path,
    slide_pdf_path: &Path,
) -> Result<VisualAlignment, VisualAlignmentError> {
    align_video_to_slides_impl(video_path, slide_pdf_path, None)
}

/// Aligns a video while reporting how much video time has been processed.
pub fn align_video_to_slides_with_progress(
    video_path: &Path,
    slide_pdf_path: &Path,
    mut report_progress: impl FnMut(VisualAlignmentProgress),
) -> Result<VisualAlignment, VisualAlignmentError> {
    let total_ms = video_duration_ms(video_path)?;
    align_video_to_slides_impl(
        video_path,
        slide_pdf_path,
        Some((&mut report_progress, total_ms)),
    )
}

fn align_video_to_slides_impl(
    video_path: &Path,
    slide_pdf_path: &Path,
    progress: Option<(&mut dyn FnMut(VisualAlignmentProgress), u64)>,
) -> Result<VisualAlignment, VisualAlignmentError> {
    let rendered_pages = TempDir::new().map_err(VisualAlignmentError::TemporaryDirectory)?;
    let slides = render_slides(slide_pdf_path, rendered_pages.path())?;
    match_video_frames(video_path, &slides, progress)
}

fn video_duration_ms(video_path: &Path) -> Result<u64, VisualAlignmentError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(video_path)
        .output()
        .map_err(|source| VisualAlignmentError::CouldNotStart {
            program: "ffprobe",
            source,
        })?;
    if !output.status.success() {
        return Err(VisualAlignmentError::CommandFailed {
            program: "ffprobe",
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let duration = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|_| VisualAlignmentError::InvalidVideoDuration)?;
    if !duration.is_finite() || duration <= 0.0 || duration > u64::MAX as f64 / 1_000.0 {
        return Err(VisualAlignmentError::InvalidVideoDuration);
    }
    Ok((duration * 1_000.0).round() as u64)
}

fn render_slides(
    slide_pdf_path: &Path,
    output_directory: &Path,
) -> Result<Vec<RenderedSlide>, VisualAlignmentError> {
    let output_prefix = output_directory.join("slide");
    let output = Command::new("pdftoppm")
        .args([
            OsStr::new("-png"),
            OsStr::new("-scale-to-x"),
            OsStr::new("320"),
            OsStr::new("-scale-to-y"),
            OsStr::new("180"),
        ])
        .arg(slide_pdf_path)
        .arg(&output_prefix)
        .output()
        .map_err(|source| VisualAlignmentError::CouldNotStart {
            program: "pdftoppm",
            source,
        })?;
    if !output.status.success() {
        return Err(VisualAlignmentError::CommandFailed {
            program: "pdftoppm",
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let mut page_paths: Vec<PathBuf> = fs::read_dir(output_directory)
        .map_err(VisualAlignmentError::ReadRenderedPages)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    page_paths.sort();
    if page_paths.is_empty() {
        return Err(VisualAlignmentError::NoRenderedPages);
    }

    page_paths
        .into_iter()
        .enumerate()
        .map(|(position, path)| {
            let id = u32::try_from(position)
                .map(SlideId)
                .map_err(|_| VisualAlignmentError::TooManySlides)?;
            let image = image::open(&path)
                .map_err(|source| VisualAlignmentError::ReadRenderedPage { path, source })?
                .into_luma8();
            if image.dimensions() != (FRAME_WIDTH, FRAME_HEIGHT) {
                return Err(VisualAlignmentError::UnexpectedPageDimensions {
                    slide_id: id,
                    width: image.width(),
                    height: image.height(),
                });
            }
            Ok(RenderedSlide { id, image })
        })
        .collect()
}

fn match_video_frames(
    video_path: &Path,
    slides: &[RenderedSlide],
    mut progress: Option<(&mut dyn FnMut(VisualAlignmentProgress), u64)>,
) -> Result<VisualAlignment, VisualAlignmentError> {
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(video_path)
        .args([
            "-vf",
            "fps=1,scale=320:180:flags=lanczos,format=gray",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| VisualAlignmentError::CouldNotStart {
            program: "ffmpeg",
            source,
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(VisualAlignmentError::MissingFfmpegOutput)?;
    let mut frame_bytes = vec![0_u8; FRAME_BYTES];
    let mut frame_matches = Vec::new();

    while read_frame(&mut stdout, &mut frame_bytes)? {
        let frame = GrayImage::from_raw(FRAME_WIDTH, FRAME_HEIGHT, frame_bytes)
            .ok_or(VisualAlignmentError::InvalidFrameDimensions)?;
        let timestamp_ms = u64::try_from(frame_matches.len())
            .ok()
            .and_then(|position| position.checked_mul(SAMPLE_PERIOD_MS))
            .ok_or(VisualAlignmentError::VideoTooLong)?;
        frame_matches.push(match_frame(timestamp_ms, &frame, slides)?);
        if let Some((report_progress, total_ms)) = &mut progress {
            report_progress(VisualAlignmentProgress {
                processed_ms: timestamp_ms.saturating_add(SAMPLE_PERIOD_MS).min(*total_ms),
                total_ms: *total_ms,
            });
        }
        frame_bytes = frame.into_raw();
    }
    drop(stdout);

    let mut stderr = String::new();
    if let Some(mut child_stderr) = child.stderr.take() {
        child_stderr
            .read_to_string(&mut stderr)
            .map_err(VisualAlignmentError::ReadFfmpegError)?;
    }
    let status = child.wait().map_err(VisualAlignmentError::WaitForFfmpeg)?;
    if !status.success() {
        return Err(VisualAlignmentError::CommandFailed {
            program: "ffmpeg",
            status,
            stderr: stderr.trim().to_owned(),
        });
    }

    Ok(VisualAlignment {
        frame_width: FRAME_WIDTH,
        frame_height: FRAME_HEIGHT,
        sample_period_ms: SAMPLE_PERIOD_MS,
        frame_matches,
    })
}

fn read_frame(reader: &mut impl Read, frame: &mut [u8]) -> Result<bool, VisualAlignmentError> {
    let mut read = 0;
    while read < frame.len() {
        match reader.read(&mut frame[read..]) {
            Ok(0) if read == 0 => return Ok(false),
            Ok(0) => {
                return Err(VisualAlignmentError::TruncatedFrame {
                    actual_bytes: read,
                    expected_bytes: frame.len(),
                });
            }
            Ok(count) => read += count,
            Err(source) => return Err(VisualAlignmentError::ReadFrame(source)),
        }
    }
    Ok(true)
}

fn match_frame(
    timestamp_ms: u64,
    frame: &GrayImage,
    slides: &[RenderedSlide],
) -> Result<FrameSlideMatch, VisualAlignmentError> {
    let mut similarities = Vec::with_capacity(slides.len());
    for slide in slides {
        let similarity =
            image_compare::gray_similarity_structure(&Algorithm::MSSIMSimple, frame, &slide.image)
                .map_err(VisualAlignmentError::CompareImages)?;
        similarities.push(SlideSimilarity {
            slide_id: slide.id,
            score: similarity.score,
        });
    }
    similarities.sort_by(|left, right| right.score.total_cmp(&left.score));
    let best = similarities
        .first()
        .copied()
        .ok_or(VisualAlignmentError::NoRenderedPages)?;
    let runner_up = similarities.get(1).copied();

    Ok(FrameSlideMatch {
        timestamp_ms,
        best,
        runner_up,
    })
}

struct RenderedSlide {
    id: SlideId,
    image: GrayImage,
}

#[derive(Debug)]
pub enum VisualAlignmentError {
    TemporaryDirectory(io::Error),
    CouldNotStart {
        program: &'static str,
        source: io::Error,
    },
    CommandFailed {
        program: &'static str,
        status: ExitStatus,
        stderr: String,
    },
    ReadRenderedPages(io::Error),
    ReadRenderedPage {
        path: PathBuf,
        source: image::ImageError,
    },
    NoRenderedPages,
    TooManySlides,
    UnexpectedPageDimensions {
        slide_id: SlideId,
        width: u32,
        height: u32,
    },
    MissingFfmpegOutput,
    ReadFrame(io::Error),
    TruncatedFrame {
        actual_bytes: usize,
        expected_bytes: usize,
    },
    InvalidFrameDimensions,
    VideoTooLong,
    InvalidVideoDuration,
    ReadFfmpegError(io::Error),
    WaitForFfmpeg(io::Error),
    CompareImages(image_compare::CompareError),
}

impl fmt::Display for VisualAlignmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TemporaryDirectory(error) => {
                write!(
                    formatter,
                    "could not create temporary slide directory: {error}"
                )
            }
            Self::CouldNotStart { program, source } => {
                write!(formatter, "could not start {program}: {source}")
            }
            Self::CommandFailed {
                program,
                status,
                stderr,
            } => write!(formatter, "{program} failed with {status}: {stderr}"),
            Self::ReadRenderedPages(error) => {
                write!(formatter, "could not list rendered PDF pages: {error}")
            }
            Self::ReadRenderedPage { path, source } => {
                write!(
                    formatter,
                    "could not read rendered page {}: {source}",
                    path.display()
                )
            }
            Self::NoRenderedPages => write!(formatter, "pdftoppm rendered no PDF pages"),
            Self::TooManySlides => write!(formatter, "PDF has more pages than can be assigned IDs"),
            Self::UnexpectedPageDimensions {
                slide_id,
                width,
                height,
            } => write!(
                formatter,
                "rendered slide {} is {width}x{height}, expected {FRAME_WIDTH}x{FRAME_HEIGHT}",
                u64::from(slide_id.0) + 1
            ),
            Self::MissingFfmpegOutput => write!(formatter, "ffmpeg did not expose frame output"),
            Self::ReadFrame(error) => write!(formatter, "could not read an ffmpeg frame: {error}"),
            Self::TruncatedFrame {
                actual_bytes,
                expected_bytes,
            } => write!(
                formatter,
                "ffmpeg returned a truncated frame with {actual_bytes} of {expected_bytes} bytes"
            ),
            Self::InvalidFrameDimensions => {
                write!(formatter, "ffmpeg returned an invalid grayscale frame")
            }
            Self::VideoTooLong => {
                write!(formatter, "video has more samples than can be timestamped")
            }
            Self::InvalidVideoDuration => {
                write!(formatter, "ffprobe returned an invalid video duration")
            }
            Self::ReadFfmpegError(error) => {
                write!(formatter, "could not read ffmpeg diagnostics: {error}")
            }
            Self::WaitForFfmpeg(error) => write!(formatter, "could not wait for ffmpeg: {error}"),
            Self::CompareImages(error) => write!(
                formatter,
                "could not compare video and slide images: {error}"
            ),
        }
    }
}

impl Error for VisualAlignmentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TemporaryDirectory(error)
            | Self::ReadRenderedPages(error)
            | Self::ReadFrame(error)
            | Self::ReadFfmpegError(error)
            | Self::WaitForFfmpeg(error) => Some(error),
            Self::CouldNotStart { source, .. } => Some(source),
            Self::ReadRenderedPage { source, .. } => Some(source),
            Self::CompareImages(error) => Some(error),
            Self::CommandFailed { .. }
            | Self::NoRenderedPages
            | Self::TooManySlides
            | Self::UnexpectedPageDimensions { .. }
            | Self::MissingFfmpegOutput
            | Self::TruncatedFrame { .. }
            | Self::InvalidFrameDimensions
            | Self::VideoTooLong
            | Self::InvalidVideoDuration => None,
        }
    }
}
