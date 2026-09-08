//! Read-only ETA for a worker's existing ASR observations, including old workers.
use indicatif::ProgressBar;

use super::transcription::Progress;

#[derive(Default)]
pub(super) struct RecognitionEstimator {
    current: Option<Estimate>,
}

struct Estimate {
    attempt: u64,
    total: u64,
    baseline: u64,
    completed: u64,
    bar: ProgressBar,
}

impl RecognitionEstimator {
    /// Only newly observed speech duration contributes to speed. Polling the
    /// same observation never counts its completed regions a second time.
    pub fn observe(&mut self, progress: Option<&Progress>, running: bool) -> Option<u64> {
        let Some(progress) = progress.filter(|p| running && !p.reused && p.phase == "recognizing")
        else {
            self.current = None;
            return None;
        };
        let Some(total) = progress
            .total_speech_ms
            .filter(|&total| total > progress.completed_speech_ms)
        else {
            self.current = None;
            return None;
        };
        let completed = progress.completed_speech_ms;
        if self.current.as_ref().is_none_or(|e| {
            e.attempt != progress.attempt_started_ms || e.total != total || completed < e.completed
        }) {
            let bar = ProgressBar::hidden();
            bar.set_length(total - completed);
            self.current = Some(Estimate {
                attempt: progress.attempt_started_ms,
                total,
                baseline: completed,
                completed,
                bar,
            });
            return None;
        }
        let estimate = self.current.as_mut().unwrap();
        estimate.completed = completed;
        estimate.bar.set_position(completed - estimate.baseline);
        (completed > estimate.baseline).then(|| estimate.bar.eta().as_millis() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_since_attachment_is_the_only_work_used_by_indicatif() {
        let mut estimator = RecognitionEstimator::default();
        let mut progress = Progress {
            phase: "recognizing".into(),
            attempt_started_ms: 1,
            completed_speech_ms: 1_804_000,
            total_speech_ms: Some(6_277_000),
            ..Progress::default()
        };
        assert_eq!(estimator.observe(Some(&progress), true), None);
        assert_eq!(estimator.observe(Some(&progress), true), None);
        assert_eq!(estimator.current.as_ref().unwrap().bar.position(), 0);
        progress.completed_speech_ms += 1000;
        assert!(estimator.observe(Some(&progress), true).is_some());
        let estimate = estimator.current.as_ref().unwrap();
        assert_eq!(estimate.bar.position(), 1000);
        assert_eq!(estimate.bar.length(), Some(6_277_000 - 1_804_000));
        assert!(
            estimator
                .observe(Some(&progress), true)
                .unwrap()
                .abs_diff(estimator.current.as_ref().unwrap().bar.eta().as_millis() as u64)
                < 100
        );
        assert_eq!(estimator.current.as_ref().unwrap().bar.position(), 1000);
        progress.phase = "finalizing".into();
        assert_eq!(estimator.observe(Some(&progress), true), None);
        assert!(estimator.current.is_none());
    }

    #[test]
    fn restarts_pauses_invalid_totals_and_reuse_never_keep_an_old_estimate() {
        let mut estimator = RecognitionEstimator::default();
        let mut progress = Progress {
            phase: "recognizing".into(),
            attempt_started_ms: 1,
            completed_speech_ms: 10,
            total_speech_ms: Some(1000),
            ..Progress::default()
        };
        estimator.observe(Some(&progress), true);
        progress.completed_speech_ms = 20;
        assert!(estimator.observe(Some(&progress), true).is_some());
        progress.attempt_started_ms = 2;
        assert_eq!(estimator.observe(Some(&progress), true), None);
        progress.completed_speech_ms = 5;
        assert_eq!(estimator.observe(Some(&progress), true), None);
        progress.total_speech_ms = Some(2000);
        assert_eq!(estimator.observe(Some(&progress), true), None);
        assert_eq!(estimator.observe(Some(&progress), false), None);
        assert!(estimator.current.is_none());
        progress.reused = true;
        assert_eq!(estimator.observe(Some(&progress), true), None);
        assert!(estimator.current.is_none());
        progress.reused = false;
        for total in [None, Some(0), Some(5), Some(4)] {
            progress.total_speech_ms = total;
            assert_eq!(estimator.observe(Some(&progress), true), None);
            assert!(estimator.current.is_none());
        }
    }
}
