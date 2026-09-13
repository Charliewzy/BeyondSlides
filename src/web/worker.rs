use std::{error::Error, ffi::OsStr, path::Path, time::Instant};

use beyond_slides::{
    ComparativeRankingError, LectureAnalysisError, RestorationSessionError, TokenBudgetError,
};

use super::jobs::{ElapsedCheckpoint, Outcome, OutcomeStatus, now_ms, read_job, worker_lock};
use crate::run_support::write_json_atomically;

pub(crate) async fn run(directory: &OsStr) -> Result<(), Box<dyn Error>> {
    if std::env::var_os("BEYOND_SLIDES_CAPTURED_WORKER").as_deref() == Some(OsStr::new("1")) {
        return execute(directory).await;
    }
    // The log-draining supervisor is independent of the web server, just like
    // the worker itself. Only its child executes the lecture pipeline.
    let job_path = Path::new(directory);
    let job = read_job(job_path)?;
    let run = job.runs.last().ok_or("lecture job has no configured run")?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("application-worker")
        .arg(directory)
        .env("BEYOND_SLIDES_CAPTURED_WORKER", "1")
        .stdin(std::process::Stdio::null());
    let key = std::env::var("BEYOND_SLIDES_API_KEY").unwrap_or_default();
    let status = super::logs::capture(
        &mut command,
        &run.directory(job_path).join("worker-debug.log"),
        &key,
    )
    .await?;
    if !status.success() {
        return Err(format!("Processing worker exited: {status}").into());
    }
    Ok(())
}

async fn execute(directory: &OsStr) -> Result<(), Box<dyn Error>> {
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
    let elapsed_path = run_directory.join("control/elapsed.json");
    let elapsed_before_ms = run.elapsed_before_ms;
    let started_ms = run.started_ms;
    let heartbeat = tokio::spawn(async move {
        loop {
            if let Err(error) = write_json_atomically(
                &elapsed_path,
                &ElapsedCheckpoint {
                    started_ms,
                    elapsed_ms: elapsed_before_ms
                        .saturating_add(started.elapsed().as_millis() as u64),
                },
                "worker elapsed checkpoint",
            ) {
                eprintln!("Could not checkpoint worker elapsed time: {error}");
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
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
    heartbeat.abort();
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
        || error_chain_contains_token_budget(error)
}

fn error_chain_contains_token_budget(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.downcast_ref::<TokenBudgetError>().is_some() {
            return true;
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_budget_errors_pause_even_when_wrapped() {
        let error = beyond_slides::ChatCompletionsError::TokenBudget(TokenBudgetError::Reached {
            limit: 100,
            consumed: 110,
        });

        assert!(is_stopped(&error));
    }
}
