use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use beyond_slides::ReportSlideImage;

const SLIDE_IMAGE_WIDTH: &str = "960";

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

    let rendered_pages = tempfile::tempdir()?;
    let output_prefix = rendered_pages.path().join("slide");
    let output = Command::new("pdftoppm")
        .args([
            OsStr::new("-png"),
            OsStr::new("-scale-to-x"),
            OsStr::new(SLIDE_IMAGE_WIDTH),
            OsStr::new("-scale-to-y"),
            OsStr::new("-1"),
        ])
        .arg(pdf_path)
        .arg(&output_prefix)
        .output()
        .map_err(|source| {
            io::Error::new(source.kind(), format!("could not start pdftoppm: {source}"))
        })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "pdftoppm failed for {} with {}: {}",
            pdf_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let mut rendered_paths: Vec<_> = fs::read_dir(rendered_pages.path())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        })
        .collect();
    rendered_paths.sort_by_key(|path| rendered_page_number(path).unwrap_or(usize::MAX));
    if rendered_paths.len() != expected_slide_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "slide PDF contains {} pages but the normalized slide deck contains {expected_slide_count} slides",
                rendered_paths.len()
            ),
        ));
    }

    let mut images = Vec::with_capacity(rendered_paths.len());
    for (index, rendered_path) in rendered_paths.into_iter().enumerate() {
        let file_name = format!("slide-{:04}.png", index + 1);
        let target = slide_directory.join(&file_name);
        fs::copy(&rendered_path, &target).map_err(|source| {
            io::Error::new(
                source.kind(),
                format!(
                    "could not copy rendered slide to {}: {source}",
                    target.display()
                ),
            )
        })?;
        let (width, height) = image::image_dimensions(&target).map_err(|source| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "could not read rendered slide {}: {source}",
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
