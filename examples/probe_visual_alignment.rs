use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::PathBuf,
    time::Instant,
};

use beyond_slides::evaluation::{VisualAlignmentProgress, align_video_to_slides_with_progress};

const PROGRESS_BAR_WIDTH: usize = 30;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    let [video_path, slide_pdf_path, output_path] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cargo run --release --example probe_visual_alignment -- \
             <lecture.mp4> <slides.pdf> <alignment.json>",
        )
        .into());
    };
    let video_path = PathBuf::from(video_path);
    let slide_pdf_path = PathBuf::from(slide_pdf_path);
    let output_path = PathBuf::from(output_path);

    eprintln!("Rendering slides, then matching one video frame per second...");
    let started = Instant::now();
    let mut last_percentage = None;
    let mut last_progress = None;
    let alignment =
        align_video_to_slides_with_progress(&video_path, &slide_pdf_path, |progress| {
            let percentage = percentage(progress);
            if last_percentage != Some(percentage) {
                draw_progress(progress, percentage);
                last_percentage = Some(percentage);
            }
            last_progress = Some(progress);
        })?;
    if let (Some(last_percentage), Some(last_progress)) = (last_percentage, last_progress) {
        if last_percentage < 100 {
            draw_progress(
                VisualAlignmentProgress {
                    processed_ms: last_progress.total_ms,
                    total_ms: last_progress.total_ms,
                },
                100,
            );
        }
        eprintln!();
    }
    let elapsed = started.elapsed();

    let mut json = serde_json::to_string_pretty(&alignment)?;
    json.push('\n');
    fs::write(&output_path, json).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", output_path.display()),
        )
    })?;
    println!(
        "Wrote {} frame matches to {} in {:.1?}",
        alignment.frame_matches.len(),
        output_path.display(),
        elapsed,
    );
    Ok(())
}

fn percentage(progress: VisualAlignmentProgress) -> u64 {
    ((u128::from(progress.processed_ms) * 100) / u128::from(progress.total_ms)) as u64
}

fn draw_progress(progress: VisualAlignmentProgress, percentage: u64) {
    let filled = usize::try_from(percentage)
        .unwrap_or(100)
        .saturating_mul(PROGRESS_BAR_WIDTH)
        / 100;
    let bar = format!(
        "{}{}",
        "#".repeat(filled),
        "-".repeat(PROGRESS_BAR_WIDTH - filled)
    );
    eprint!(
        "\r[{bar}] {percentage:>3}%  {} / {}",
        display_time(progress.processed_ms),
        display_time(progress.total_ms),
    );
    let _ = io::stderr().flush();
}

fn display_time(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}
