use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

use fs2::FileExt;
use rapidocr_core::{
    RapidOcr,
    config::{LimitType, PipelineConfig},
    model::{ModelAssetSpec, PPOCRV5_CH_MOBILE},
    types::OcrOutput,
};
use reqwest::Client;

use super::model_assets::download_verified;
use beyond_slides::ingestion::pdf;

const MODEL_DIRECTORY: &str = "pp-ocr-v5-chinese-mobile";
const DETECTION_MODEL_BYTES: u64 = 4_819_576;
const RECOGNITION_MODEL_BYTES: u64 = 16_631_306;
const DICTIONARY_BYTES: u64 = 74_012;

pub(super) struct ModelPaths {
    directory: PathBuf,
}

pub(super) async fn ensure_models(
    application_root: &Path,
    mut report: impl FnMut(u64, u64),
) -> Result<ModelPaths, String> {
    let directory = application_root.join("models").join(MODEL_DIRECTORY);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(directory.join("download.lock"))
        .map_err(io_error)?;
    lock_file.lock_exclusive().map_err(io_error)?;

    let pipeline = PipelineConfig {
        use_det: true,
        use_cls: false,
        use_rec: true,
    };
    let assets = PPOCRV5_CH_MOBILE.assets_for_pipeline(pipeline);
    let mut total = 0_u64;
    for asset in &assets {
        total += asset_bytes(*asset)?;
    }
    let client = Client::builder()
        .user_agent(concat!("BeyondSlides/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(provider_error)?;
    let mut completed = 0;
    for asset in assets {
        let size = asset_bytes(asset)?;
        let checksum = asset
            .sha256
            .ok_or_else(|| format!("{} has no integrity checksum", asset.name))?;
        download_verified(
            &client,
            asset.url,
            size,
            checksum,
            &directory.join(asset.filename),
            |downloaded| {
                report(completed + downloaded, total);
                Ok(())
            },
        )
        .await
        .map_err(provider_error)?;
        completed += size;
    }
    report(total, total);
    Ok(ModelPaths { directory })
}

pub(super) fn recognize_pages(
    pages: &[PathBuf],
    models: ModelPaths,
    mut report: impl FnMut(usize, usize),
) -> Result<Vec<String>, String> {
    let mut engine = engine(models)?;
    let mut texts = Vec::with_capacity(pages.len());
    report(0, pages.len());
    for (index, page) in pages.iter().enumerate() {
        let output = engine.run_path(page).map_err(|error| {
            format!(
                "could not recognize Rain Classroom courseware page {}: {error}",
                index + 1
            )
        })?;
        texts.push(output_text(output));
        report(index + 1, pages.len());
    }
    Ok(texts)
}

/// Reads each PDF page at a time so a large uploaded deck does not hold all
/// rendered pages in memory while its text is recognized.
pub(super) fn recognize_pdf(
    path: &Path,
    page_count: usize,
    page_indices: &[usize],
    models: ModelPaths,
    mut report: impl FnMut(usize, usize),
) -> Result<Vec<String>, String> {
    let mut engine = engine(models)?;
    let mut texts = vec![String::new(); page_count];
    report(0, page_indices.len());
    for (completed, &index) in page_indices.iter().enumerate() {
        let image = pdf::render_page_to_fit(path, index, 1600).map_err(|error| {
            format!(
                "could not render uploaded slide page {}: {error}",
                index + 1
            )
        })?;
        let output = engine.run_image(&image.to_rgb8()).map_err(|error| {
            format!(
                "could not recognize uploaded slide page {}: {error}",
                index + 1
            )
        })?;
        texts[index] = output_text(output);
        report(completed + 1, page_indices.len());
    }
    Ok(texts)
}

fn engine(models: ModelPaths) -> Result<RapidOcr, String> {
    let pipeline = PipelineConfig {
        use_det: true,
        use_cls: false,
        use_rec: true,
    };
    let mut config = PPOCRV5_CH_MOBILE
        .config(models.directory)
        .with_pipeline(pipeline);
    config.inference.intra_threads = std::thread::available_parallelism()
        .map(|threads| threads.get().min(4))
        .unwrap_or(1);
    // Match the official PP-OCRv5 mobile detection preprocessing used by the
    // prototype instead of the crate's cross-model defaults.
    let detector = config
        .det
        .as_mut()
        .expect("the selected OCR pipeline includes detection");
    detector.limit_side_len = 960;
    detector.limit_type = LimitType::Max;
    detector.unclip_ratio = 1.5;

    RapidOcr::new(config).map_err(provider_error)
}

fn output_text(output: OcrOutput) -> String {
    output
        .lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn asset_bytes(asset: ModelAssetSpec) -> Result<u64, String> {
    match asset.filename {
        "ch_PP-OCRv5_det_mobile.onnx" => Ok(DETECTION_MODEL_BYTES),
        "ch_PP-OCRv5_rec_mobile.onnx" => Ok(RECOGNITION_MODEL_BYTES),
        "ppocrv5_dict.txt" => Ok(DICTIONARY_BYTES),
        filename => Err(format!("unexpected PP-OCRv5 mobile asset {filename}")),
    }
}

fn io_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn provider_error(error: impl std::fmt::Display) -> String {
    format!("could not prepare native slide OCR: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "set BEYOND_SLIDES_OCR_SMOKE_ROOT to a directory containing page.jpg"]
    async fn recognizes_a_real_slide_with_downloaded_native_models() {
        let root = std::env::var_os("BEYOND_SLIDES_OCR_SMOKE_ROOT")
            .map(PathBuf::from)
            .expect("set BEYOND_SLIDES_OCR_SMOKE_ROOT");
        let models = ensure_models(&root, |_, _| {}).await.unwrap();
        let texts = recognize_pages(&[root.join("page.jpg")], models, |_, _| {}).unwrap();

        assert_eq!(texts.len(), 1);
        assert!(
            texts[0]
                .chars()
                .filter(|character| !character.is_whitespace())
                .count()
                >= 8
        );
    }
}
