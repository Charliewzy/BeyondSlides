use std::{error::Error, ffi::OsStr, path::Path, time::Instant};

use beyond_slides::{ComparativeRankingError, LectureAnalysisError, RestorationSessionError};

use super::jobs::{Outcome, OutcomeStatus, now_ms, read_job, worker_lock};
use crate::run_support::write_json_atomically;

pub(crate) async fn run(directory: &OsStr) -> Result<(), Box<dyn Error>> {
    let directory = Path::new(directory);
    let lock = worker_lock(directory)?;
    lock.try_lock()?;
    let mut job = read_job(directory)?;
    let run = job
        .runs
        .last()
        .ok_or("lecture job has no configured run")?
        .clone();
    let run_directory = run.directory(directory);
    let started = Instant::now();
    let result = async {
        super::transcription::prepare(directory, &run_directory, &mut job).await?;
        let recording = job
            .recording
            .as_ref()
            .filter(|_| job.preview.duration_ms.is_some())
            .map(|file| directory.join(file));
        let timing = directory.join("timed-tokens.json");
        crate::analysis_run::run_complete(
            directory.join("transcript.json").as_os_str(),
            directory.join("slides.json").as_os_str(),
            run_directory.join("analysis").as_os_str(),
            Some(directory.join("slides.pdf").as_os_str()),
            recording.as_deref().map(Path::as_os_str),
            timing.is_file().then_some(timing.as_os_str()),
        )
        .await
    }
    .await;
    let status = match &result {
        Ok(()) => OutcomeStatus::Complete,
        Err(error) if is_stopped(error.as_ref()) => OutcomeStatus::Paused,
        Err(_) => OutcomeStatus::Failed,
    };
    let key = std::env::var("BEYOND_SLIDES_API_KEY").unwrap_or_default();
    let error = result.err().map(|e| {
        let message = e.to_string();
        let message = if key.is_empty() {
            message
        } else {
            message.replace(&key, "<REDACTED>")
        };
        message.chars().take(4000).collect()
    });
    write_json_atomically(
        &run_directory.join("outcome.json"),
        &Outcome {
            status,
            error,
            finished_ms: now_ms(),
            elapsed_ms: run
                .elapsed_before_ms
                .saturating_add(started.elapsed().as_millis() as u64),
        },
        "worker outcome",
    )?;
    Ok(())
}

fn is_stopped(error: &(dyn Error + 'static)) -> bool {
    matches!(
        error.downcast_ref::<RestorationSessionError>(),
        Some(RestorationSessionError::Stopped)
    ) || matches!(
        error.downcast_ref::<LectureAnalysisError>(),
        Some(LectureAnalysisError::Stopped)
    ) || matches!(
        error.downcast_ref::<ComparativeRankingError>(),
        Some(ComparativeRankingError::Stopped)
    ) || error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::Interrupted)
}
