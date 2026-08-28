use std::{env, error::Error, fs, io, path::PathBuf};

use beyond_slides::ingestion::pdf::import;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [input_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --example import_pdf -- <slides.pdf> <slides.json>",
        )
        .into());
    };
    let input_path = PathBuf::from(input_path);
    let output_path = PathBuf::from(output_path);

    let imported = import(&input_path)?;
    let mut json = serde_json::to_string_pretty(&imported.slide_deck)?;
    json.push('\n');
    fs::write(&output_path, json).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", output_path.display()),
        )
    })?;

    println!(
        "Wrote {} slides to {}",
        imported.slide_deck.slides.len(),
        output_path.display()
    );
    for warning in imported.warnings {
        eprintln!("Warning: {warning:?}");
    }
    Ok(())
}
