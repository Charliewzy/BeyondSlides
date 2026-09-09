use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use beyond_slides::{ReportAudio, ReportSlideImage, ingestion::pdf};

const SLIDE_IMAGE_WIDTH: u32 = 960;
const AUDIO_ASSET_STEM: &str = "lecture-audio";

/// Makes a recording available beside the report without duplicating it when
/// the source and report are on the same filesystem.
pub fn prepare_audio_asset(
    audio_path: &Path,
    report_path: &Path,
) -> Result<ReportAudio, io::Error> {
    let metadata = fs::metadata(audio_path).map_err(|source| {
        io::Error::new(
            source.kind(),
            format!(
                "could not read report audio {}: {source}",
                audio_path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("report audio {} is not a file", audio_path.display()),
        ));
    }

    let report_parent = report_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let report_stem = report_path.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("report path {} has no file name", report_path.display()),
        )
    })?;
    let asset_directory_name = format!("{}.assets", report_stem.to_string_lossy());
    let asset_directory = report_parent.join(&asset_directory_name);
    fs::create_dir_all(&asset_directory)?;

    let file_name = audio_path.extension().map_or_else(
        || AUDIO_ASSET_STEM.into(),
        |extension| format!("{AUDIO_ASSET_STEM}.{}", extension.to_string_lossy()),
    );
    let target = asset_directory.join(&file_name);
    let already_staged = target.exists() && audio_path.canonicalize()? == target.canonicalize()?;
    if !already_staged {
        // Secure the replacement before touching any existing report assets.
        let staging = tempfile::tempdir_in(&asset_directory)?;
        let staged = staging.path().join(&file_name);
        if let Err(link_error) = fs::hard_link(audio_path, &staged) {
            fs::copy(audio_path, &staged).map_err(|copy_error| {
                io::Error::new(
                    copy_error.kind(),
                    format!(
                        "could not hard-link {} ({link_error}) or copy it to {} ({copy_error})",
                        audio_path.display(),
                        target.display()
                    ),
                )
            })?;
        }
        fs::rename(&staged, &target)?;
    }
    remove_stale_audio_assets(&asset_directory, &target)?;

    Ok(ReportAudio {
        source: PathBuf::from(asset_directory_name)
            .join(file_name)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    })
}

/// Renders one report-local PNG for every PDF page in presentation order.
pub fn render_pdf_slides(
    pdf_path: &Path,
    report_path: &Path,
    expected_slide_count: usize,
) -> Result<Vec<ReportSlideImage>, io::Error> {
    let report_parent = report_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let report_stem = report_path.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("report path {} has no file name", report_path.display()),
        )
    })?;
    let asset_directory_name = format!("{}.assets", report_stem.to_string_lossy());
    let slide_directory = report_parent.join(&asset_directory_name).join("slides");
    fs::create_dir_all(&slide_directory).map_err(|source| {
        io::Error::new(
            source.kind(),
            format!(
                "could not create report slide directory {}: {source}",
                slide_directory.display()
            ),
        )
    })?;

    let rendered_pages =
        pdf::render_pages_to_width(pdf_path, SLIDE_IMAGE_WIDTH).map_err(io::Error::other)?;
    if rendered_pages.len() != expected_slide_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "slide PDF contains {} pages but the normalized slide deck contains {expected_slide_count} slides",
                rendered_pages.len()
            ),
        ));
    }

    let mut images = Vec::with_capacity(rendered_pages.len());
    for (index, rendered_page) in rendered_pages.into_iter().enumerate() {
        let file_name = format!("slide-{:04}.png", index + 1);
        let target = slide_directory.join(&file_name);
        let width = rendered_page.width();
        let height = rendered_page.height();
        rendered_page
            .save_with_format(&target, image::ImageFormat::Png)
            .map_err(|source| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "could not save rendered slide {}: {source}",
                        target.display()
                    ),
                )
            })?;
        let relative_source = PathBuf::from(&asset_directory_name)
            .join("slides")
            .join(file_name)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        images.push(ReportSlideImage {
            source: relative_source,
            width,
            height,
        });
    }

    remove_stale_slides(&slide_directory, expected_slide_count)?;
    Ok(images)
}

fn rendered_page_number(path: &Path) -> Option<usize> {
    path.file_stem()
        .and_then(OsStr::to_str)
        .and_then(|stem| stem.rsplit_once('-'))
        .and_then(|(_, number)| number.parse().ok())
}

fn remove_stale_slides(directory: &Path, slide_count: usize) -> Result<(), io::Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let Some(number) = rendered_page_number(&path) else {
            continue;
        };
        let is_generated_png = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
            && path
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(|stem| stem.starts_with("slide-"));
        if is_generated_png && number > slide_count {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn remove_stale_audio_assets(directory: &Path, retained: &Path) -> Result<(), io::Error> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if entry.path() != retained
            && (file_name == AUDIO_ASSET_STEM || file_name.starts_with("lecture-audio."))
            && entry.file_type()?.is_file()
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerender_preserves_audio_already_in_the_assets_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let assets = directory.path().join("report.assets");
        fs::create_dir(&assets)?;
        let input = assets.join("lecture-audio.flac");
        fs::write(&input, b"only recording copy")?;
        prepare_audio_asset(&input, &directory.path().join("report.html"))?;
        prepare_audio_asset(
            &assets.join("./lecture-audio.flac"),
            &directory.path().join("report.html"),
        )?;
        assert_eq!(fs::read(&input)?, b"only recording copy");
        Ok(())
    }

    #[test]
    fn audio_replacement_keeps_old_asset_until_new_input_is_available()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let report = directory.path().join("report.html");
        let input = directory.path().join("original.flac");
        fs::write(&input, b"original")?;
        prepare_audio_asset(&input, &report)?;
        let old = directory.path().join("report.assets/lecture-audio.flac");
        assert!(prepare_audio_asset(&directory.path().join("missing.mp3"), &report).is_err());
        assert_eq!(fs::read(&old)?, b"original");
        let replacement = directory.path().join("replacement.mp3");
        fs::write(&replacement, b"replacement")?;
        prepare_audio_asset(&replacement, &report)?;
        assert!(!old.exists());
        assert_eq!(
            fs::read(directory.path().join("report.assets/lecture-audio.mp3"))?,
            b"replacement"
        );
        assert_eq!(fs::read(&input)?, b"original");
        Ok(())
    }

    #[test]
    fn pdf_pages_become_ordered_report_assets() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let report_path = directory.path().join("lecture.html");
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/visual_alignment.pdf");

        let images = render_pdf_slides(&fixture, &report_path, 2)?;

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].source, "lecture.assets/slides/slide-0001.png");
        assert_eq!(images[1].source, "lecture.assets/slides/slide-0002.png");
        assert_eq!((images[0].width, images[0].height), (960, 540));
        assert!(directory.path().join(&images[0].source).exists());
        assert!(directory.path().join(&images[1].source).exists());
        Ok(())
    }

    #[test]
    fn audio_becomes_a_report_local_asset() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio_path = directory.path().join("recording.flac");
        let report_path = directory.path().join("lecture.html");
        fs::write(&audio_path, b"audio fixture")?;

        let audio = prepare_audio_asset(&audio_path, &report_path)?;

        assert_eq!(audio.source, "lecture.assets/lecture-audio.flac");
        assert_eq!(
            fs::read(directory.path().join(audio.source))?,
            b"audio fixture"
        );
        Ok(())
    }
}
