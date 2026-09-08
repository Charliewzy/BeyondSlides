use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use serde::Serialize;
use tokio::{sync::Notify, time::Instant};

const MAX_LEARNED_INTERVAL: Duration = Duration::from_secs(120);

/// Shared admission for one endpoint/quota scope. Clone across clients using
/// the same quota; unrelated providers should use separate schedulers.
#[derive(Clone)]
pub struct RequestScheduler(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestSchedulingSnapshot {
    pub adaptive: bool,
    pub initial_concurrency: usize,
    pub max_concurrency: usize,
    pub effective_concurrency: usize,
    pub request_interval_ms: u64,
    pub cooldown_remaining_ms: u64,
    pub in_flight: usize,
    pub waiting: usize,
    pub peak_in_flight: usize,
    pub requests_started: u64,
    pub successful_responses: u64,
    pub rate_limited_responses: u64,
    pub failed_responses: u64,
    pub cancelled_requests: u64,
    pub total_admission_wait_ms: u64,
}

struct State {
    adaptive: bool,
    initial: usize,
    ceiling: usize,
    limit: usize,
    floor: Duration,
    learned_interval: Duration,
    next_start: Instant,
    cooldown: Instant,
    generation: u64,
    healthy_since: Instant,
    healthy_successes: usize,
    latency_seconds: f64,
    concurrency_blocked: bool,
    pacing_blocked: bool,
    in_flight: usize,
    waiting: usize,
    peak: usize,
    started: u64,
    successful: u64,
    throttled: u64,
    failed: u64,
    cancelled: u64,
    wait_ms: u64,
}

pub(crate) enum RequestFeedback {
    Success,
    RateLimited { retry_after: Duration },
    Failed { retry_after: Option<Duration> },
}

impl RequestScheduler {
    pub fn fixed(ceiling: NonZeroUsize, minimum_interval: Duration) -> Self {
        Self::new(ceiling, ceiling, minimum_interval, false)
    }

    /// Begins at the configured initial concurrency (or the smaller ceiling),
    /// grows on healthy queued demand, and reduces concurrency/rate on HTTP 429.
    pub fn adaptive(ceiling: NonZeroUsize, minimum_interval: Duration) -> Self {
        Self::adaptive_starting_at(
            NonZeroUsize::new(2).expect("two is non-zero"),
            ceiling,
            minimum_interval,
        )
    }

    pub fn adaptive_starting_at(
        initial_concurrency: NonZeroUsize,
        ceiling: NonZeroUsize,
        minimum_interval: Duration,
    ) -> Self {
        Self::new(ceiling, initial_concurrency, minimum_interval, true)
    }

    fn new(
        ceiling: NonZeroUsize,
        initial_concurrency: NonZeroUsize,
        floor: Duration,
        adaptive: bool,
    ) -> Self {
        let now = Instant::now();
        let initial = if adaptive {
            initial_concurrency.get().min(ceiling.get())
        } else {
            ceiling.get()
        };
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                adaptive,
                initial,
                ceiling: ceiling.get(),
                limit: initial,
                floor,
                learned_interval: Duration::ZERO,
                next_start: now,
                cooldown: now,
                generation: 0,
                healthy_since: now,
                healthy_successes: 0,
                latency_seconds: 1.0,
                concurrency_blocked: false,
                pacing_blocked: false,
                in_flight: 0,
                waiting: 0,
                peak: 0,
                started: 0,
                successful: 0,
                throttled: 0,
                failed: 0,
                cancelled: 0,
                wait_ms: 0,
            }),
            changed: Notify::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> RequestSchedulingSnapshot {
        let state = self.lock();
        RequestSchedulingSnapshot {
            adaptive: state.adaptive,
            initial_concurrency: state.initial,
            max_concurrency: state.ceiling,
            effective_concurrency: state.limit,
            request_interval_ms: milliseconds(state.interval()),
            cooldown_remaining_ms: milliseconds(
                state.cooldown.saturating_duration_since(Instant::now()),
            ),
            in_flight: state.in_flight,
            waiting: state.waiting,
            peak_in_flight: state.peak,
            requests_started: state.started,
            successful_responses: state.successful,
            rate_limited_responses: state.throttled,
            failed_responses: state.failed,
            cancelled_requests: state.cancelled,
            total_admission_wait_ms: state.wait_ms,
        }
    }

    pub(crate) async fn acquire(&self) -> RequestPermit {
        let queued_at = Instant::now();
        self.lock().waiting += 1;
        let _waiting = Waiting(self.clone());
        loop {
            // Register before checking the predicate; every wake rechecks both
            // capacity and deadlines, including cooldowns extended by peers.
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let deadline = {
                let mut state = self.lock();
                let now = Instant::now();
                let ready_at = state.next_start.max(state.cooldown);
                if state.in_flight < state.limit && now >= ready_at {
                    state.in_flight += 1;
                    state.started += 1;
                    state.peak = state.peak.max(state.in_flight);
                    state.wait_ms = state.wait_ms.saturating_add(milliseconds(now - queued_at));
                    state.next_start = now.checked_add(state.interval()).unwrap_or(now);
                    return RequestPermit {
                        scheduler: self.clone(),
                        generation: state.generation,
                        started: now,
                        finished: false,
                    };
                }
                if now >= state.cooldown {
                    state.concurrency_blocked |= state.in_flight >= state.limit;
                    state.pacing_blocked |= state.in_flight < state.limit && now < state.next_start;
                }
                (state.in_flight < state.limit).then_some(ready_at)
            };
            match deadline {
                Some(deadline) => tokio::select! {
                    _ = &mut changed => {},
                    _ = tokio::time::sleep_until(deadline) => {},
                },
                None => changed.await,
            }
        }
    }
}

struct Waiting(RequestScheduler);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.lock().waiting -= 1;
    }
}

pub(crate) struct RequestPermit {
    scheduler: RequestScheduler,
    generation: u64,
    started: Instant,
    finished: bool,
}

impl RequestPermit {
    pub(crate) fn finish(mut self, feedback: RequestFeedback) {
        let now = Instant::now();
        self.scheduler
            .lock()
            .observe(self.generation, feedback, now - self.started, now);
        self.finished = true;
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        {
            let mut state = self.scheduler.lock();
            state.in_flight -= 1;
            state.cancelled += u64::from(!self.finished);
        }
        self.scheduler.0.changed.notify_waiters();
    }
}

impl State {
    fn interval(&self) -> Duration {
        self.floor.max(self.learned_interval)
    }

    fn reset_health(&mut self, now: Instant) {
        self.healthy_successes = 0;
        self.healthy_since = now.max(self.cooldown);
        self.concurrency_blocked = false;
        self.pacing_blocked = false;
    }

    fn extend_cooldown(&mut self, now: Instant, delay: Duration) {
        if let Some(deadline) = now.checked_add(delay) {
            self.cooldown = self.cooldown.max(deadline);
        }
    }

    fn observe(
        &mut self,
        generation: u64,
        feedback: RequestFeedback,
        elapsed: Duration,
        now: Instant,
    ) {
        match feedback {
            RequestFeedback::RateLimited { retry_after } => {
                self.throttled += 1;
                self.extend_cooldown(now, retry_after);
                self.reset_health(now);
                if self.adaptive && generation == self.generation {
                    self.generation += 1;
                    self.limit = (self.limit / 2).max(1);
                    let seconds = (2.0 * self.interval().as_secs_f64())
                        .max(self.latency_seconds / self.limit as f64)
                        .max(0.25)
                        .min(MAX_LEARNED_INTERVAL.as_secs_f64());
                    self.learned_interval = Duration::from_secs_f64(seconds);
                }
            }
            RequestFeedback::Success => {
                self.successful += 1;
                if !self.adaptive || generation != self.generation {
                    return;
                }
                self.latency_seconds = 0.8 * self.latency_seconds + 0.2 * elapsed.as_secs_f64();
                self.healthy_successes += 1;
                if self.healthy_successes < 8.max(self.limit.saturating_mul(2))
                    || now.saturating_duration_since(self.healthy_since) < Duration::from_secs(1)
                    || now < self.cooldown
                {
                    return;
                }
                if self.waiting > 0 {
                    if self.pacing_blocked
                        && self.interval().as_secs_f64() > self.latency_seconds / self.limit as f64
                    {
                        self.relax_spacing();
                    } else if self.concurrency_blocked {
                        self.limit = self.ceiling.min(self.limit.saturating_add(1));
                    } else if self.pacing_blocked {
                        self.relax_spacing();
                    }
                }
                self.reset_health(now);
            }
            RequestFeedback::Failed { retry_after } => {
                self.failed += 1;
                if let Some(delay) = retry_after {
                    self.extend_cooldown(now, delay);
                }
                // Unavailability is not evidence of an RPM quota.
                self.reset_health(now);
            }
        }
    }

    fn relax_spacing(&mut self) {
        self.learned_interval = self.learned_interval.mul_f64(0.8);
        if self.learned_interval < Duration::from_millis(10) {
            self.learned_interval = Duration::ZERO;
        }
    }
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler(adaptive: bool, ceiling: usize) -> RequestScheduler {
        RequestScheduler::new(
            NonZeroUsize::new(ceiling).unwrap(),
            NonZeroUsize::new(2).unwrap(),
            Duration::ZERO,
            adaptive,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn queued_requests_exercise_real_admission_and_recovery() {
        use futures::{StreamExt, stream};
        let gate = scheduler(true, 4);
        stream::iter(0..48)
            .for_each_concurrent(12, |_| async {
                let permit = gate.acquire().await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                permit.finish(RequestFeedback::Success);
            })
            .await;
        let final_state = gate.snapshot();
        assert_eq!(final_state.requests_started, 48);
        assert_eq!(final_state.successful_responses, 48);
        assert_eq!(final_state.effective_concurrency, 4);
        assert_eq!(final_state.peak_in_flight, 4);
        assert_eq!(final_state.waiting, 0);
        assert_eq!(final_state.in_flight, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn permits_bound_attempts_and_release_on_cancellation() {
        let gate = scheduler(false, 1);
        let permit = gate.acquire().await;
        let cloned = gate.clone();
        let waiter = tokio::spawn(async move { cloned.acquire().await });
        tokio::task::yield_now().await;
        assert_eq!(gate.snapshot().waiting, 1);
        assert!(!waiter.is_finished());
        waiter.abort();
        let _ = waiter.await;
        assert_eq!(gate.snapshot().waiting, 0);
        drop(permit);
        let next = gate.acquire().await;
        assert_eq!(gate.snapshot().in_flight, 1);
        assert_eq!(gate.snapshot().cancelled_requests, 1);
        next.finish(RequestFeedback::Success);
        assert_eq!(gate.snapshot().in_flight, 0);
        assert_eq!(gate.snapshot().peak_in_flight, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_waiters_do_not_reserve_future_pacing_slots() {
        let gate = RequestScheduler::fixed(NonZeroUsize::new(4).unwrap(), Duration::from_secs(10));
        let start = Instant::now();
        gate.acquire().await.finish(RequestFeedback::Success);
        let cloned = gate.clone();
        let waiter = tokio::spawn(async move { cloned.acquire().await });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        gate.acquire().await.finish(RequestFeedback::Success);
        assert_eq!(Instant::now() - start, Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_throttle_extends_all_waiters_without_a_second_reduction() {
        let gate = scheduler(true, 8);
        let first = gate.acquire().await;
        let second = gate.acquire().await;
        first.finish(RequestFeedback::RateLimited {
            retry_after: Duration::from_secs(4),
        });
        let cloned = gate.clone();
        let waiter = tokio::spawn(async move { cloned.acquire().await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(2)).await;
        second.finish(RequestFeedback::RateLimited {
            retry_after: Duration::from_secs(6),
        });
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        assert_eq!(gate.snapshot().effective_concurrency, 1);
        assert_eq!(gate.snapshot().request_interval_ms, 1000);
        tokio::time::advance(Duration::from_secs(3)).await;
        waiter.await.unwrap().finish(RequestFeedback::Success);
        assert_eq!(gate.snapshot().requests_started, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn long_server_cooldowns_are_not_clipped_and_expiry_is_paced() {
        let gate = scheduler(true, 4);
        let start = Instant::now();
        gate.acquire().await.finish(RequestFeedback::RateLimited {
            retry_after: Duration::from_secs(600),
        });
        gate.acquire().await.finish(RequestFeedback::Success);
        assert_eq!(Instant::now() - start, Duration::from_secs(600));
        gate.acquire().await.finish(RequestFeedback::Success);
        assert_eq!(Instant::now() - start, Duration::from_secs(601));
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_throttling_at_cap_one_still_reduces_the_send_rate() {
        let gate = scheduler(true, 1);
        for _ in 0..4 {
            gate.acquire().await.finish(RequestFeedback::RateLimited {
                retry_after: Duration::ZERO,
            });
        }
        assert_eq!(gate.snapshot().effective_concurrency, 1);
        assert_eq!(gate.snapshot().request_interval_ms, 8000);
    }

    #[tokio::test(start_paused = true)]
    async fn no_growth_without_demand_and_stale_successes_cannot_undo_backoff() {
        let gate = scheduler(true, 8);
        for _ in 0..24 {
            let permit = gate.acquire().await;
            tokio::time::advance(Duration::from_secs(1)).await;
            permit.finish(RequestFeedback::Success);
        }
        assert_eq!(gate.snapshot().effective_concurrency, 2);
        let mut state = gate.lock();
        let now = Instant::now();
        state.observe(
            0,
            RequestFeedback::RateLimited {
                retry_after: Duration::ZERO,
            },
            Duration::ZERO,
            now,
        );
        state.waiting = 4;
        state.concurrency_blocked = true;
        for _ in 0..40 {
            state.observe(
                0,
                RequestFeedback::Success,
                Duration::from_secs(1),
                now + Duration::from_secs(2),
            );
        }
        assert_eq!(state.limit, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_queued_demand_grows_slowly_and_respects_the_ceiling() {
        let gate = scheduler(true, 4);
        let mut state = gate.lock();
        let start = Instant::now();
        state.waiting = 10;
        for second in 1..100 {
            state.concurrency_blocked = true;
            state.observe(
                0,
                RequestFeedback::Success,
                Duration::from_secs(1),
                start + Duration::from_secs(second),
            );
            if second < 8 {
                assert_eq!(state.limit, 2);
            }
            assert!(state.limit <= 4);
        }
        assert_eq!(state.limit, 4);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_failures_do_not_learn_a_quota_and_floor_is_authoritative() {
        let gate =
            RequestScheduler::adaptive(NonZeroUsize::new(8).unwrap(), Duration::from_secs(5));
        gate.acquire().await.finish(RequestFeedback::Failed {
            retry_after: Some(Duration::from_secs(10)),
        });
        assert_eq!(gate.snapshot().effective_concurrency, 2);
        assert_eq!(gate.snapshot().request_interval_ms, 5000);
        let mut state = gate.lock();
        state.learned_interval = Duration::from_secs(6);
        state.relax_spacing();
        assert_eq!(state.interval(), Duration::from_secs(5));
    }

    #[test]
    fn adaptive_scheduler_uses_the_configured_initial_concurrency() {
        let scheduler = RequestScheduler::adaptive_starting_at(
            NonZeroUsize::new(5).unwrap(),
            NonZeroUsize::new(8).unwrap(),
            Duration::ZERO,
        );
        let snapshot = scheduler.snapshot();
        assert_eq!(snapshot.initial_concurrency, 5);
        assert_eq!(snapshot.effective_concurrency, 5);
        assert_eq!(snapshot.max_concurrency, 8);
    }

    #[test]
    fn adaptive_initial_concurrency_is_capped_by_the_ceiling() {
        let scheduler = RequestScheduler::adaptive_starting_at(
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(3).unwrap(),
            Duration::ZERO,
        );
        let snapshot = scheduler.snapshot();
        assert_eq!(snapshot.initial_concurrency, 3);
        assert_eq!(snapshot.effective_concurrency, 3);
    }
}
