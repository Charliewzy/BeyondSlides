use std::{env, error::Error, fs, io, path::Path};

use beyond_slides::{
    SlideDeck, SlideId, SlideScore,
    evaluation::{AlignmentVisualizationWindow, VisualAlignment, render_alignment_visualization},
};
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Deserialize)]
struct SemanticAlignmentArtifact {
    windows: Vec<WindowAlignment>,
}

#[derive(Deserialize)]
struct WindowAlignment {
    number: usize,
    owned_region: OwnedRegion,
    query: String,
    slide_scores: Vec<SlideScore>,
    slide_position: SlideId,
}

#[derive(Deserialize)]
struct OwnedRegion {
    start_ms: u64,
    end_ms: u64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let (slides_path, semantic_path, visual_path, output_path) = match arguments.as_slice() {
        [slides_path, semantic_path, output_path] => {
            (slides_path, semantic_path, None, output_path)
        }
        [slides_path, semantic_path, visual_path, output_path] => {
            (slides_path, semantic_path, Some(visual_path), output_path)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: cargo run --release --example visualize_alignment -- \
                 <slides.json> <semantic-alignment.json> [visual-alignment.json] <report.html>",
            )
            .into());
        }
    };

    let slide_deck: SlideDeck = read_json(Path::new(slides_path))?;
    let semantic: SemanticAlignmentArtifact = read_json(Path::new(semantic_path))?;
    let visual: Option<VisualAlignment> = visual_path
        .map(|path| read_json(Path::new(path)))
        .transpose()?;
    let windows: Vec<_> = semantic
        .windows
        .iter()
        .map(|window| AlignmentVisualizationWindow {
            number: window.number,
            start_ms: window.owned_region.start_ms,
            end_ms: window.owned_region.end_ms,
            transcript: window.query.as_str(),
            slide_scores: &window.slide_scores,
            slide_position: window.slide_position,
        })
        .collect();
    let html = render_alignment_visualization(&slide_deck, &windows, visual.as_ref())?;
    fs::write(output_path, html).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not write alignment report {}: {error}",
                Path::new(output_path).display()
            ),
        )
    })?;
    println!(
        "Wrote {} alignment windows to {}",
        windows.len(),
        Path::new(output_path).display()
    );
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not read {}: {error}", path.display()),
        )
    })?;
    serde_json::from_str(&contents).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not parse {}: {error}", path.display()),
        )
        .into()
    })
}
