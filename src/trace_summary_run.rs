use std::{error::Error, ffi::OsStr, path::PathBuf};

use beyond_slides::evaluation::{render_model_trace_summary, summarize_model_trace};

pub fn run(trace_path: &OsStr) -> Result<(), Box<dyn Error>> {
    let trace_path = PathBuf::from(trace_path);
    let summary = summarize_model_trace(&trace_path)?;
    print!("{}", render_model_trace_summary(&summary));
    Ok(())
}
