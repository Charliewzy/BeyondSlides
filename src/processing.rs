//! Cooperative stopping at checkpointable work-item boundaries.
use std::{
    error::Error,
    fmt,
    future::Future,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures::{StreamExt, stream};

/// A one-way request to stop admitting work, not to cancel in-flight requests.
/// Use a fresh signal when resuming a stopped run.
#[derive(Clone, Debug, Default)]
pub struct StopSignal(Arc<AtomicBool>);

impl StopSignal {
    pub fn request_stop(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub enum BatchRunError<E> {
    Stopped,
    Work(E),
}

impl<E: fmt::Display> fmt::Display for BatchRunError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("processing stopped; completed checkpoints preserved"),
            Self::Work(error) => error.fmt(f),
        }
    }
}

impl<E: Error + 'static> Error for BatchRunError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Stopped => None,
            Self::Work(error) => Some(error),
        }
    }
}

/// Runs checkpointable work with bounded concurrency. Stop requests and errors
/// halt admission; already-admitted work is drained and every successful result
/// is delivered to `save`. The first error wins over a simultaneous stop request.
///
/// `execute` encompasses a complete conversation, including its bounded retries
/// and repairs. Dropping this future is *not* a graceful stop.
pub async fn run_bounded<I, T, E, F: Future<Output = Result<T, E>>>(
    items: impl IntoIterator<Item = I>,
    concurrency: NonZeroUsize,
    stop: &StopSignal,
    execute: impl FnMut(I) -> F,
    mut save: impl FnMut(T) -> Result<(), E>,
) -> Result<(), BatchRunError<E>> {
    let failed = AtomicBool::new(false);
    let work = stream::iter(items)
        .take_while(|_| std::future::ready(!stop.is_requested() && !failed.load(Ordering::Acquire)))
        .map(execute)
        .buffer_unordered(concurrency.get());
    futures::pin_mut!(work);
    let mut first_error = None;
    while let Some(result) = work.next().await {
        if let Err(error) = result.and_then(&mut save) {
            failed.store(true, Ordering::Release);
            first_error.get_or_insert(error);
        }
    }
    match first_error {
        Some(error) => Err(BatchRunError::Work(error)),
        None if stop.is_requested() => Err(BatchRunError::Stopped),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Mutex, time::Duration};

    #[tokio::test(start_paused = true)]
    async fn stopping_drains_admitted_work_and_saves_it() {
        let stop = StopSignal::default();
        let started = Mutex::new(Vec::new());
        let mut saved = Vec::new();
        let result = run_bounded(
            0..10,
            NonZeroUsize::new(2).unwrap(),
            &stop,
            |index| {
                started.lock().unwrap().push(index);
                async move {
                    tokio::time::sleep(Duration::from_secs(index + 1)).await;
                    Ok::<_, &'static str>(index)
                }
            },
            |index| {
                saved.push(index);
                stop.request_stop();
                Ok(())
            },
        )
        .await;
        assert!(matches!(result, Err(BatchRunError::Stopped)));
        assert_eq!(*started.lock().unwrap(), [0, 1]);
        assert_eq!(saved, [0, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn errors_also_drain_successful_siblings() {
        let mut saved = Vec::new();
        let result = run_bounded(
            0..10,
            NonZeroUsize::new(2).unwrap(),
            &StopSignal::default(),
            |index| async move {
                tokio::time::sleep(Duration::from_secs(index + 1)).await;
                if index == 0 {
                    Err("provider failed")
                } else {
                    Ok(index)
                }
            },
            |index| {
                saved.push(index);
                Ok(())
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(BatchRunError::Work("provider failed"))
        ));
        assert_eq!(saved, [1]);
    }

    #[tokio::test]
    async fn pre_stopped_run_never_executes_work() {
        let stop = StopSignal::default();
        stop.request_stop();
        let result = run_bounded(
            [0],
            NonZeroUsize::new(1).unwrap(),
            &stop,
            |_| async {
                panic!("must not start");
                #[allow(unreachable_code)]
                Ok::<_, &str>(())
            },
            |_| Ok(()),
        )
        .await;
        assert!(matches!(result, Err(BatchRunError::Stopped)));
    }
}
