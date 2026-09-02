use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use beyond_slides::{
    Transcript, TranscriptWindowRestorationResult, evaluation::render_restoration_review,
};

use crate::run_support::{read_json, write_text_atomically};

pub fn run(
    transcript_path: &OsStr,
    run_directory: &OsStr,
    output_path: &OsStr,
) -> Result<(), Box<dyn Error>> {
    let transcript_path = PathBuf::from(transcript_path);
    let run_directory = PathBuf::from(run_directory);
    let output_path = PathBuf::from(output_path);
    let transcript: Transcript = read_json(&transcript_path, "transcript")?;
    let window_results = read_window_results(&run_directory)?;
    let html = render_restoration_review(&transcript, &window_results)?;
    write_text_atomically(&output_path, &html, "restoration review")?;
    println!(
        "Wrote restoration review for {} windows to {}",
        window_results.len(),
        output_path.display()
    );
    Ok(())
}

fn read_window_results(
    run_directory: &Path,
) -> Result<Vec<TranscriptWindowRestorationResult>, io::Error> {
    let entries = fs::read_dir(run_directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not read restoration run directory {}: {error}",
                run_directory.display()
            ),
        )
    })?;
    let mut checkpoints = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(window_number) = checkpoint_number(&path) else {
            continue;
        };
        if checkpoints.insert(window_number, path.clone()).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "restoration run directory {} has duplicate checkpoint number {window_number}",
                    run_directory.display()
                ),
            ));
        }
    }

    checkpoints
        .into_iter()
        .enumerate()
        .map(|(window_index, (window_number, path))| {
            let expected = window_index + 1;
            if window_number != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "restoration checkpoints skip window {expected}; next checkpoint is {window_number}"
                    ),
                ));
            }
            read_json(&path, "restoration window checkpoint")
        })
        .collect()
}

fn checkpoint_number(path: &Path) -> Option<usize> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("window-")?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_names_are_recognized_strictly() {
        assert_eq!(checkpoint_number(Path::new("window-0007.json")), Some(7));
        assert_eq!(checkpoint_number(Path::new("window-seven.json")), None);
        assert_eq!(checkpoint_number(Path::new("diagnostics.json")), None);
    }
}
