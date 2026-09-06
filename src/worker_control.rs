//! Machine-readable worker status. Never includes model prompts or credentials.
use std::{collections::BTreeMap, env, io, path::PathBuf, time::Duration};

use beyond_slides::processing::StopSignal;
use serde::{Deserialize, Serialize};

use crate::run_support::{read_json, write_json_atomically};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    Transcription,
    Restoration,
    Retrieval,
    Passages,
    Comparisons,
    Rendering,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StageProgress {
    pub completed: usize,
    pub total: Option<usize>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct WorkerProgress {
    pub current: Option<Stage>,
    pub stages: BTreeMap<Stage, StageProgress>,
}

pub(crate) struct WorkerControl {
    directory: Option<PathBuf>,
    stop: StopSignal,
    monitor: Option<tokio::task::JoinHandle<()>>,
}

impl WorkerControl {
    pub fn from_environment() -> Result<Self, io::Error> {
        let directory = env::var_os("BEYOND_SLIDES_WORKER_CONTROL").map(PathBuf::from);
        Self::new(directory)
    }

    pub fn new(directory: Option<PathBuf>) -> Result<Self, io::Error> {
        let stop = StopSignal::default();
        let monitor = if let Some(directory) = &directory {
            std::fs::create_dir_all(directory)?;
            let path = directory.join("stop-requested");
            if path.exists() {
                stop.request_stop();
            }
            let stop = stop.clone();
            Some(tokio::spawn(async move {
                loop {
                    if path.exists() {
                        stop.request_stop();
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }))
        } else {
            None
        };
        Ok(Self {
            directory,
            stop,
            monitor,
        })
    }

    pub fn stop_signal(&self) -> StopSignal {
        // Also check synchronously before admission, including immediately after
        // long blocking CPU preparation that may have delayed the monitor.
        if self
            .directory
            .as_ref()
            .is_some_and(|d| d.join("stop-requested").exists())
        {
            self.stop.request_stop();
        }
        self.stop.clone()
    }

    pub fn check_stop(&self) -> Result<(), io::Error> {
        if self.stop_signal().is_requested() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "processing stopped; completed checkpoints preserved",
            ))
        } else {
            Ok(())
        }
    }

    pub fn progress(
        &self,
        stage: Stage,
        completed: usize,
        total: Option<usize>,
    ) -> Result<(), io::Error> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        let path = directory.join("progress.json");
        let mut progress: WorkerProgress = if path.exists() {
            read_json(&path, "worker progress")?
        } else {
            WorkerProgress::default()
        };
        progress.current = Some(stage);
        progress
            .stages
            .insert(stage, StageProgress { completed, total });
        write_json_atomically(&path, &progress, "worker progress")
    }
}

impl Drop for WorkerControl {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_is_persistent_and_stop_is_seen_before_work_starts()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let control = WorkerControl::new(Some(directory.path().into()))?;
        control.progress(Stage::Restoration, 2, Some(5))?;
        control.progress(Stage::Passages, 0, None)?;
        let progress: WorkerProgress =
            read_json(&directory.path().join("progress.json"), "progress")?;
        assert_eq!(progress.stages[&Stage::Restoration].completed, 2);
        assert_eq!(progress.stages[&Stage::Passages].total, None);
        std::fs::write(directory.path().join("stop-requested"), [])?;
        assert!(control.stop_signal().is_requested());
        assert_eq!(
            control.check_stop().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        Ok(())
    }
}
