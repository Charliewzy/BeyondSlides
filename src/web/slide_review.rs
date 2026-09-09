//! Optional source inspection, independent of analysis runs and checkpoints.
use std::{
    io,
    path::{Path, PathBuf},
};

use beyond_slides::{
    SlideDeck,
    ingestion::pdf::{ImportWarning, page_text_warnings},
};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::run_support::read_json;

#[derive(Serialize)]
pub(super) struct ReviewPage {
    page: u32,
    text: String,
    warnings: Vec<String>,
}

pub(super) fn pages(directory: &Path) -> io::Result<Vec<ReviewPage>> {
    let deck: SlideDeck = read_json(&contained(directory, "slides.json")?, "slides")?;
    Ok(deck
        .slides
        .iter()
        .enumerate()
        .filter_map(|(index, slide)| {
            let page = u32::try_from(index).ok()?.checked_add(1)?;
            let warnings: Vec<_> = page_text_warnings(page, &slide.text)
                .iter()
                .map(|warning| match warning {
                    ImportWarning::SparseText {
                        non_whitespace_characters,
                        ..
                    } => format!("仅提取到 {non_whitespace_characters} 个字符（不计空白）"),
                    ImportWarning::SuspiciousGlyphs { glyphs, .. } => {
                        format!("疑似异常字符：{}", glyphs.iter().collect::<String>())
                    }
                })
                .collect();
            (!warnings.is_empty()).then(|| ReviewPage {
                page,
                text: slide.text.clone(),
                warnings,
            })
        })
        .collect())
}

fn contained(directory: &Path, name: &str) -> io::Result<PathBuf> {
    let path = directory.join(name).canonicalize()?;
    if !path.starts_with(directory.canonicalize()?) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Preview must remain inside its lecture directory",
        ));
    }
    Ok(path)
}

/// Render only a flagged page. A dedicated cache never touches report assets.
pub(super) async fn image(directory: &Path, page: u32, renders: &Semaphore) -> io::Result<Vec<u8>> {
    if !pages(directory)?
        .iter()
        .any(|candidate| candidate.page == page)
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Unknown review page",
        ));
    }
    let _permit = renders.acquire().await.map_err(io::Error::other)?;
    let cache = directory.join("slide-review");
    tokio::fs::create_dir_all(&cache).await?;
    let cache = contained(directory, "slide-review")?;
    let filename = format!("page-{page:04}.png");
    let target = cache.join(&filename);
    if target.exists() {
        return tokio::fs::read(contained(&cache, &filename)?).await;
    }
    let temporary = tempfile::tempdir_in(&cache)?;
    let staged = temporary.path().join("page.png");
    let pdf = contained(directory, "slides.pdf")?;
    tokio::task::spawn_blocking(move || {
        let image = beyond_slides::ingestion::pdf::render_page_to_fit(
            &pdf,
            usize::try_from(page - 1).map_err(io::Error::other)?,
            1200,
        )
        .map_err(io::Error::other)?;
        image
            .save_with_format(&staged, image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        Ok::<_, io::Error>(staged)
    })
    .await
    .map_err(io::Error::other)??;
    tokio::fs::rename(temporary.path().join("page.png"), &target).await?;
    tokio::fs::read(target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saved_deck_review_is_ordered_and_only_flagged_pages_can_be_rendered() -> io::Result<()>
    {
        let directory = tempfile::tempdir()?;
        let imported =
            beyond_slides::ingestion::pdf::import(Path::new("tests/fixtures/pdf_import.pdf"))
                .map_err(io::Error::other)?;
        std::fs::write(
            directory.path().join("slides.json"),
            serde_json::to_vec(&imported.slide_deck)?,
        )?;
        std::fs::copy(
            "tests/fixtures/pdf_import.pdf",
            directory.path().join("slides.pdf"),
        )?;
        let pages = pages(directory.path())?;
        assert_eq!(pages.iter().map(|p| p.page).collect::<Vec<_>>(), vec![3]);
        assert!(pages.windows(2).all(|pair| pair[0].page < pair[1].page));
        let renders = Semaphore::new(2);
        assert_eq!(
            image(directory.path(), 1, &renders)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            image(directory.path(), 0, &renders)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            image(directory.path(), u32::MAX, &renders)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        let png = image(directory.path(), pages[0].page, &renders).await?;
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        std::fs::rename(
            directory.path().join("slides.pdf"),
            directory.path().join("original.pdf"),
        )?;
        assert_eq!(png, image(directory.path(), pages[0].page, &renders).await?);
        assert!(!directory.path().join("run-0001").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn preview_sources_cannot_escape_the_job_directory() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        std::os::unix::fs::symlink(outside.path(), directory.path().join("slides.json"))?;
        assert_eq!(
            pages(directory.path()).err().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        Ok(())
    }
}
