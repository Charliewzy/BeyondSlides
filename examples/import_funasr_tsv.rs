use std::{env, error::Error, fs, io, path::PathBuf};

use beyond_slides::ingestion::funasr::import_tsv;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [input_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --example import_funasr_tsv -- <input.tsv> <transcript.json>",
        )
        .into());
    };
    let input_path = PathBuf::from(input_path);
    let output_path = PathBuf::from(output_path);

    let input = fs::read_to_string(&input_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", input_path.display()),
        )
    })?;
    let transcript = import_tsv(&input)?;
    let mut json = serde_json::to_string_pretty(&transcript)?;
    json.push('\n');
    fs::write(&output_path, json).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", output_path.display()),
        )
    })?;

    println!(
        "Wrote {} transcript sentences to {}",
        transcript.sentences.len(),
        output_path.display()
    );
    Ok(())
}
