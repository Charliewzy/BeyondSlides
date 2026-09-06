use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use zip::{ZipWriter, write::SimpleFileOptions};

/// Export an allowlisted reader bundle, never the surrounding run directory.
pub(super) fn build(analysis: &Path, destination: &Path) -> Result<(), io::Error> {
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::other("export needs a parent directory"))?;
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut archive = ZipWriter::new(output.as_file_mut());
        append(&mut archive, analysis, "report.html")?;
        let assets = analysis.join("report.assets");
        if assets.is_dir() {
            for entry in fs::read_dir(&assets)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("lecture-audio.") && entry.file_type()?.is_file() {
                    append(&mut archive, analysis, &format!("report.assets/{name}"))?;
                }
            }
            let slides = assets.join("slides");
            if slides.is_dir() {
                for entry in fs::read_dir(slides)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".png") && entry.file_type()?.is_file() {
                        append(
                            &mut archive,
                            analysis,
                            &format!("report.assets/slides/{name}"),
                        )?;
                    }
                }
            }
        }
        archive.start_file(
            "README.txt",
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        )?;
        archive.write_all("解压整个 ZIP 后，用浏览器打开 report.html。请保留 report.assets 文件夹。\nExtract the whole ZIP and open report.html in a browser. Keep report.assets beside it.\nThis bundle contains lecture text, slides, and any attached recording. Share only with permission.\n".as_bytes())?;
        archive.finish()?;
    }
    output.as_file().sync_all()?;
    output.persist(destination).map_err(|e| e.error)?;
    Ok(())
}

fn append(
    archive: &mut ZipWriter<&mut fs::File>,
    root: &Path,
    name: &str,
) -> Result<(), io::Error> {
    let path = root.join(name);
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(io::Error::other("Refusing to export a symbolic link"));
    }
    if !path.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(io::Error::other(
            "Export asset is outside the report directory",
        ));
    }
    let mut file = fs::File::open(path)?;
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(file.metadata()?.len() >= u32::MAX as u64);
    archive.start_file(name, options)?;
    io::copy(&mut file, archive)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_does_not_include_private_run_files() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        fs::create_dir_all(root.join("report.assets/slides"))?;
        for file in [
            "report.html",
            "report.assets/slides/page-1.png",
            "report.assets/lecture-audio.mp3",
            "model-trace.jsonl",
            "manifest.json",
            "report.assets/private.json",
        ] {
            fs::write(root.join(file), b"test")?;
        }
        let zip = root.join("reader.zip");
        build(root, &zip)?;
        let archive = zip::ZipArchive::new(fs::File::open(zip)?)?;
        let names: std::collections::BTreeSet<_> = archive.file_names().collect();
        assert_eq!(
            names,
            [
                "README.txt",
                "report.html",
                "report.assets/slides/page-1.png",
                "report.assets/lecture-audio.mp3"
            ]
            .into_iter()
            .collect()
        );
        Ok(())
    }
}
