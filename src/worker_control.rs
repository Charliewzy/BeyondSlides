//! Machine-readable worker status. Never includes model prompts or credentials.
use std::{
    collections::BTreeMap,
    env, io,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use beyond_slides::processing::StopSignal;
use indicatif::ProgressBar;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StageProgress {
    pub completed: usize,
    pub total: Option<usize>,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub eta_ms: Option<u64>,
    #[serde(default)]
    pub reused: bool,
}

struct StageClock {
    stage: Stage,
    started: Instant,
    elapsed_before_ms: u64,
    baseline: usize,
    progress: StageProgress,
    estimate: ProgressBar,
}

impl StageClock {
    fn snapshot(&mut self) {
        self.progress.elapsed_ms = Some(
            self.elapsed_before_ms
                .saturating_add(self.started.elapsed().as_millis() as u64),
        );
        self.progress.eta_ms = (self.progress.total.is_some()
            && self.progress.completed > self.baseline)
            .then(|| self.estimate.eta().as_millis() as u64);
    }
}

struct StageTiming {
    path: PathBuf,
    active: Option<StageClock>,
}

impl StageTiming {
    fn read(&self) -> io::Result<WorkerProgress> {
        if self.path.exists() {
            read_json(&self.path, "worker progress")
        } else {
            Ok(WorkerProgress::default())
        }
    }

    fn save(&mut self, finish: bool) -> io::Result<()> {
        let mut progress = self.read()?;
        if let Some(clock) = &mut self.active {
            clock.snapshot();
            if finish {
                clock.progress.eta_ms = None;
            }
            progress.current = Some(clock.stage);
            // Serialize while holding the timing lock; heartbeat and callbacks
            // must never overwrite one another's progress.
            progress.stages.insert(clock.stage, clock.progress.clone());
            write_json_atomically(&self.path, &progress, "worker progress")?;
        }
        if finish {
            self.active = None;
        }
        Ok(())
    }
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
    timing: Option<Arc<Mutex<StageTiming>>>,
}

impl WorkerControl {
    pub fn from_environment() -> Result<Self, io::Error> {
        let directory = env::var_os("BEYOND_SLIDES_WORKER_CONTROL").map(PathBuf::from);
        Self::new(directory)
    }

    pub fn new(directory: Option<PathBuf>) -> Result<Self, io::Error> {
        let stop = StopSignal::default();
        let timing = directory.as_ref().map(|directory| {
            Arc::new(Mutex::new(StageTiming {
                path: directory.join("progress.json"),
                active: None,
            }))
        });
        let monitor = if let Some(directory) = &directory {
            std::fs::create_dir_all(directory)?;
            let path = directory.join("stop-requested");
            if path.exists() {
                stop.request_stop();
            }
            let stop = stop.clone();
            let timing = timing
                .clone()
                .expect("a control directory has timing state");
            Some(tokio::spawn(async move {
                let mut last_saved = Instant::now();
                loop {
                    if path.exists() {
                        stop.request_stop();
                    }
                    if last_saved.elapsed() >= Duration::from_secs(1) {
                        if let Err(error) = timing.lock().unwrap().save(false) {
                            eprintln!("Could not checkpoint stage timing: {error}");
                        }
                        last_saved = Instant::now();
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
            timing,
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
        self.update(stage, completed, total, false)
    }

    /// Establishes checkpoint coverage without treating restored work as speed.
    pub fn baseline(&self, stage: Stage, completed: usize, total: usize) -> io::Result<()> {
        self.update(stage, completed, Some(total), true)
    }

    fn update(
        &self,
        stage: Stage,
        completed: usize,
        total: Option<usize>,
        baseline: bool,
    ) -> io::Result<()> {
        let Some(timing) = &self.timing else {
            return Ok(());
        };
        let mut timing = timing.lock().unwrap();
        if timing.active.is_none()
            && !baseline
            && total == Some(completed)
            && timing
                .read()?
                .stages
                .get(&stage)
                .is_some_and(|s| s.total == total && s.completed == completed)
        {
            return Ok(());
        }
        if timing
            .active
            .as_ref()
            .is_none_or(|clock| clock.stage != stage)
        {
            timing.save(true)?;
            let saved = timing.read()?;
            let elapsed_before_ms = saved
                .stages
                .get(&stage)
                .and_then(|s| s.elapsed_ms)
                .unwrap_or(0);
            timing.active = Some(StageClock {
                stage,
                started: Instant::now(),
                elapsed_before_ms,
                baseline: completed,
                progress: StageProgress {
                    completed,
                    total,
                    elapsed_ms: None,
                    eta_ms: None,
                    reused: false,
                },
                estimate: ProgressBar::hidden(),
            });
        }
        let clock = timing.active.as_mut().unwrap();
        if baseline {
            clock.baseline = completed;
            clock.estimate.reset();
        }
        clock.progress.completed = completed;
        clock.progress.total = total;
        clock.progress.reused = baseline && total == Some(completed) && completed > 0;
        if let Some(total) = total {
            clock
                .estimate
                .set_length(total.saturating_sub(clock.baseline) as u64);
            clock
                .estimate
                .set_position(completed.saturating_sub(clock.baseline) as u64);
        }
        timing.save(total == Some(completed))
    }
}

impl Drop for WorkerControl {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
        if let Some(timing) = &self.timing {
            let Ok(mut timing) = timing.lock() else {
                eprintln!("Could not checkpoint final stage timing: timing lock is poisoned");
                return;
            };
            if let Err(error) = timing.save(true) {
                eprintln!("Could not checkpoint final stage timing: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_progress_has_unknown_timing() -> Result<(), serde_json::Error> {
        let progress: StageProgress = serde_json::from_str(r#"{"completed":2,"total":5}"#)?;
        assert!(progress.elapsed_ms.is_none());
        assert!(progress.eta_ms.is_none());
        assert!(!progress.reused);
        Ok(())
    }

    #[tokio::test]
    async fn resume_keeps_elapsed_but_checkpoints_do_not_contribute_to_speed() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("progress.json");
        let control = WorkerControl::new(Some(directory.path().into()))?;
        control.baseline(Stage::Restoration, 80, 100)?;
        let (position, length) = {
            let mut timing = control.timing.as_ref().unwrap().lock().unwrap();
            let clock = timing.active.as_mut().unwrap();
            clock.started -= Duration::from_secs(20);
            let position = clock.estimate.position();
            let length = clock.estimate.length();
            timing.save(false)?;
            (position, length)
        };
        assert_eq!(position, 0);
        assert_eq!(length, Some(20));
        let saved: WorkerProgress = read_json(&path, "progress")?;
        assert!(saved.stages[&Stage::Restoration].eta_ms.is_none());
        control.progress(Stage::Restoration, 81, Some(100))?;
        let (position, saved_eta, clock_eta) = {
            let timing = control.timing.as_ref().unwrap().lock().unwrap();
            let clock = timing.active.as_ref().unwrap();
            let saved: WorkerProgress = read_json(&path, "progress")?;
            (
                clock.estimate.position(),
                saved.stages[&Stage::Restoration].eta_ms,
                clock.progress.eta_ms,
            )
        };
        assert_eq!(position, 1);
        // The serialized value is exactly the Indicatif-backed stage clock's
        // snapshot; comparing two live ETA calls is timing-sensitive.
        assert_eq!(saved_eta, clock_eta);
        assert!(saved_eta.is_some());
        drop(control);
        let saved: WorkerProgress = read_json(&path, "progress")?;
        let elapsed = saved.stages[&Stage::Restoration].elapsed_ms.unwrap();
        assert!(elapsed >= 20_000);
        assert!(saved.stages[&Stage::Restoration].eta_ms.is_none());
        let resumed = WorkerControl::new(Some(directory.path().into()))?;
        resumed.baseline(Stage::Restoration, 81, 100)?;
        let saved: WorkerProgress = read_json(&path, "progress")?;
        assert!(
            saved.stages[&Stage::Restoration]
                .elapsed_ms
                .unwrap()
                .abs_diff(elapsed)
                < 1000
        );
        assert!(saved.stages[&Stage::Restoration].eta_ms.is_none());
        resumed.progress(Stage::Restoration, 100, Some(100))?;
        drop(resumed);
        let reused = WorkerControl::new(Some(directory.path().into()))?;
        reused.baseline(Stage::Restoration, 100, 100)?;
        reused.progress(Stage::Restoration, 100, Some(100))?;
        let saved: WorkerProgress = read_json(&path, "progress")?;
        assert!(saved.stages[&Stage::Restoration].reused);
        assert!(saved.stages[&Stage::Restoration].elapsed_ms.unwrap() >= elapsed);
        assert!(saved.stages[&Stage::Restoration].eta_ms.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_updates_waiting_time_and_completed_stage_stays_frozen() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let control = WorkerControl::new(Some(directory.path().into()))?;
        control.progress(Stage::Retrieval, 0, None)?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let path = directory.path().join("progress.json");
        let saved: WorkerProgress = read_json(&path, "progress")?;
        assert!(saved.stages[&Stage::Retrieval].elapsed_ms.unwrap() >= 1000);
        assert!(saved.stages[&Stage::Retrieval].eta_ms.is_none());
        control.progress(Stage::Retrieval, 1, Some(1))?;
        let saved: WorkerProgress = read_json(&path, "progress")?;
        let elapsed = saved.stages[&Stage::Retrieval].elapsed_ms;
        control.baseline(Stage::Passages, 0, 10)?;
        drop(control);
        let saved: WorkerProgress = read_json(&path, "progress")?;
        assert_eq!(saved.stages[&Stage::Retrieval].elapsed_ms, elapsed);
        Ok(())
    }

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
