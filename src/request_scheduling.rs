use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    fmt,
    future::Future,
    num::{NonZeroU64, NonZeroUsize},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use serde::Serialize;
use tokio::{sync::Notify, time::Instant};

use crate::{
    ModelRequestKind, ModelTraceEvent, ModelTraceRecord, ModelWorkflow,
    model_trace::ModelTraceContext,
};

const MAX_LEARNED_INTERVAL: Duration = Duration::from_secs(120);
const MIN_HEDGE_SAMPLES: usize = 10;
const MAX_HEDGE_SAMPLES: usize = 50;
const MAX_ACTIVE_HEDGES: usize = 3;
const MAX_HEDGES_PER_RUN: u64 = 20;
const HEDGE_LATENCY_MULTIPLIER: u32 = 3;
const HEDGE_LATENCY_OFFSET: Duration = Duration::from_secs(5);

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
    pub active_hedges: usize,
    pub hedges_started: u64,
    pub max_active_hedges: usize,
    pub max_hedges_per_run: u64,
    pub total_admission_wait_ms: u64,
    pub token_budget: Option<TokenBudgetSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenBudgetSnapshot {
    pub limit: u64,
    pub consumed: u64,
    pub missing_usage_responses: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBudgetError {
    Reached {
        limit: u64,
        consumed: u64,
    },
    UsageUnavailable {
        limit: u64,
        consumed: u64,
        missing_responses: u64,
    },
}

impl fmt::Display for TokenBudgetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reached { limit, consumed } => write!(
                formatter,
                "token budget of {limit} reached after {consumed} recorded tokens; increase or remove the budget to resume"
            ),
            Self::UsageUnavailable {
                limit,
                consumed,
                missing_responses,
            } => write!(
                formatter,
                "token budget of {limit} cannot be enforced after {consumed} recorded tokens because {missing_responses} model responses omitted input or output token usage; increase or remove the budget to resume"
            ),
        }
    }
}

impl Error for TokenBudgetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RequestClass {
    workflow: ModelWorkflow,
    request_kind: ModelRequestKind,
}

impl From<ModelTraceContext> for RequestClass {
    fn from(context: ModelTraceContext) -> Self {
        Self {
            workflow: context.workflow,
            request_kind: context.request_kind,
        }
    }
}

#[derive(Clone)]
pub(crate) struct HedgePlan {
    scheduler: RequestScheduler,
    class: RequestClass,
    logical_request_id: u64,
    pub(crate) threshold: Duration,
    pub(crate) p80: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestAttemptMetadata {
    pub(crate) logical_request_id: u64,
    pub(crate) hedge_number: Option<u64>,
    pub(crate) threshold: Option<Duration>,
    pub(crate) p80: Option<Duration>,
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
    next_logical_request_id: u64,
    active_hedges: usize,
    hedges_started: u64,
    response_latencies: HashMap<RequestClass, VecDeque<Duration>>,
    wait_ms: u64,
    token_budget: Option<TokenBudgetSnapshot>,
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
                next_logical_request_id: 0,
                active_hedges: 0,
                hedges_started: 0,
                response_latencies: HashMap::new(),
                wait_ms: 0,
                token_budget: None,
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
            active_hedges: state.active_hedges,
            hedges_started: state.hedges_started,
            max_active_hedges: MAX_ACTIVE_HEDGES,
            max_hedges_per_run: MAX_HEDGES_PER_RUN,
            total_admission_wait_ms: state.wait_ms,
            token_budget: state.token_budget,
        }
    }

    /// Applies one run-wide budget to every request admitted through this
    /// shared scheduler. Existing trace history must be restored afterward.
    pub fn with_token_budget(self, limit: NonZeroU64) -> Self {
        self.lock().token_budget = Some(TokenBudgetSnapshot {
            limit: limit.get(),
            consumed: 0,
            missing_usage_responses: 0,
        });
        self
    }

    pub(crate) fn check_token_budget(&self) -> Result<(), TokenBudgetError> {
        let state = self.lock();
        let Some(budget) = state.token_budget else {
            return Ok(());
        };
        if budget.missing_usage_responses > 0 {
            return Err(TokenBudgetError::UsageUnavailable {
                limit: budget.limit,
                consumed: budget.consumed,
                missing_responses: budget.missing_usage_responses,
            });
        }
        if budget.consumed >= budget.limit {
            return Err(TokenBudgetError::Reached {
                limit: budget.limit,
                consumed: budget.consumed,
            });
        }
        Ok(())
    }

    pub(crate) fn record_token_usage(&self, input: Option<u64>, output: Option<u64>) {
        let mut state = self.lock();
        state.record_token_usage(input, output);
    }

    #[cfg(test)]
    pub(crate) async fn acquire(&self) -> RequestPermit {
        self.acquire_class(None).await
    }

    /// Restores request identifiers, latency samples, hedge usage, and token
    /// usage from one existing trace before a resumable run admits requests.
    /// Call this once for each trace that belongs to the run.
    pub fn restore_history(&self, records: &[ModelTraceRecord]) {
        let mut state = self.lock();
        for record in records {
            let class = RequestClass {
                workflow: record.workflow,
                request_kind: record.request_kind,
            };
            match &record.event {
                ModelTraceEvent::Request {
                    logical_request_id,
                    hedge,
                    ..
                } => {
                    if let Some(logical_request_id) = logical_request_id {
                        state.next_logical_request_id = state
                            .next_logical_request_id
                            .max(logical_request_id.saturating_add(1));
                    }
                    if hedge.is_some() {
                        state.hedges_started = state.hedges_started.saturating_add(1);
                    }
                }
                ModelTraceEvent::Response {
                    elapsed_ms,
                    response,
                    ..
                } => {
                    let latencies = state.response_latencies.entry(class).or_default();
                    if latencies.len() == MAX_HEDGE_SAMPLES {
                        latencies.pop_front();
                    }
                    latencies.push_back(Duration::from_millis(*elapsed_ms));
                    state.record_token_usage(
                        response["usage"]["prompt_tokens"].as_u64(),
                        response["usage"]["completion_tokens"].as_u64(),
                    );
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn acquire_for(&self, context: ModelTraceContext) -> RequestPermit {
        self.acquire_class(Some(context.into())).await
    }

    async fn acquire_class(&self, class: Option<RequestClass>) -> RequestPermit {
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
                    let logical_request_id = state.next_logical_request_id;
                    state.next_logical_request_id = state.next_logical_request_id.saturating_add(1);
                    let hedge = class.and_then(|class| {
                        state
                            .hedge_threshold(class)
                            .map(|(p80, threshold)| HedgePlan {
                                scheduler: self.clone(),
                                class,
                                logical_request_id,
                                threshold,
                                p80,
                            })
                    });
                    return RequestPermit {
                        scheduler: self.clone(),
                        generation: state.generation,
                        started: now,
                        class,
                        hedge,
                        metadata: RequestAttemptMetadata {
                            logical_request_id,
                            hedge_number: None,
                            threshold: None,
                            p80: None,
                        },
                        is_hedge: false,
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

    /// Runs one admitted provider attempt and, once enough same-class latency
    /// evidence exists, races it against one exceptional duplicate. An error
    /// from either attempt does not defeat a still-running peer.
    pub(crate) async fn run_with_hedge<T, E, F, Fut>(
        &self,
        context: ModelTraceContext,
        request: F,
    ) -> Result<T, E>
    where
        F: Fn(RequestPermit) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let primary_permit = self.acquire_for(context).await;
        let hedge_plan = primary_permit.hedge_plan();
        let primary = request(primary_permit);
        tokio::pin!(primary);
        let Some(hedge_plan) = hedge_plan else {
            return primary.await;
        };

        let threshold = hedge_plan.threshold;
        let hedge_permit = tokio::select! {
            result = &mut primary => return result,
            permit = hedge_plan.acquire_after_threshold() => permit,
        };
        let Some(hedge_permit) = hedge_permit else {
            return primary.await;
        };
        let metadata = hedge_permit.metadata();
        eprintln!(
            "Launching hedge {}/{} for {:?}/{:?} after {} ms (p80 {} ms)",
            metadata.hedge_number.unwrap_or_default(),
            MAX_HEDGES_PER_RUN,
            context.workflow,
            context.request_kind,
            milliseconds(threshold),
            milliseconds(metadata.p80.unwrap_or_default()),
        );
        let hedge = request(hedge_permit);
        tokio::pin!(hedge);
        tokio::select! {
            primary_result = &mut primary => match primary_result {
                Ok(output) => Ok(output),
                Err(_) => hedge.await,
            },
            hedge_result = &mut hedge => match hedge_result {
                Ok(output) => Ok(output),
                Err(_) => primary.await,
            },
        }
    }
}

impl HedgePlan {
    /// Reserves one exceptional request slot without waiting behind ordinary
    /// admission. A task may call this only after its learned deadline expires.
    async fn acquire_after_threshold(self) -> Option<RequestPermit> {
        tokio::time::sleep(self.threshold).await;
        loop {
            let changed = self.scheduler.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let permit = {
                let mut state = self.scheduler.lock();
                if state.hedges_started >= MAX_HEDGES_PER_RUN {
                    return None;
                }
                if state.active_hedges >= MAX_ACTIVE_HEDGES {
                    None
                } else {
                    state.active_hedges += 1;
                    state.hedges_started += 1;
                    state.in_flight += 1;
                    state.started += 1;
                    state.peak = state.peak.max(state.in_flight);
                    let hedge_number = state.hedges_started;
                    Some(RequestPermit {
                        scheduler: self.scheduler.clone(),
                        generation: state.generation,
                        started: Instant::now(),
                        class: Some(self.class),
                        hedge: None,
                        metadata: RequestAttemptMetadata {
                            logical_request_id: self.logical_request_id,
                            hedge_number: Some(hedge_number),
                            threshold: Some(self.threshold),
                            p80: Some(self.p80),
                        },
                        is_hedge: true,
                        finished: false,
                    })
                }
            };
            if permit.is_some() {
                return permit;
            }
            changed.await;
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
    class: Option<RequestClass>,
    hedge: Option<HedgePlan>,
    metadata: RequestAttemptMetadata,
    is_hedge: bool,
    finished: bool,
}

impl RequestPermit {
    pub(crate) fn hedge_plan(&self) -> Option<HedgePlan> {
        self.hedge.clone()
    }

    pub(crate) const fn metadata(&self) -> RequestAttemptMetadata {
        self.metadata
    }

    pub(crate) fn finish(mut self, feedback: RequestFeedback) {
        let now = Instant::now();
        self.scheduler.lock().observe(
            self.generation,
            self.class,
            feedback,
            now - self.started,
            now,
        );
        self.finished = true;
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        {
            let mut state = self.scheduler.lock();
            state.in_flight -= 1;
            state.active_hedges -= usize::from(self.is_hedge);
            state.cancelled += u64::from(!self.finished);
        }
        self.scheduler.0.changed.notify_waiters();
    }
}

impl State {
    fn record_token_usage(&mut self, input: Option<u64>, output: Option<u64>) {
        let Some(budget) = self.token_budget.as_mut() else {
            return;
        };
        let Some(tokens) =
            input.and_then(|input| output.and_then(|output| input.checked_add(output)))
        else {
            budget.missing_usage_responses = budget.missing_usage_responses.saturating_add(1);
            return;
        };
        budget.consumed = budget.consumed.saturating_add(tokens);
    }

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
        class: Option<RequestClass>,
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
                if let Some(class) = class {
                    let latencies = self.response_latencies.entry(class).or_default();
                    if latencies.len() == MAX_HEDGE_SAMPLES {
                        latencies.pop_front();
                    }
                    latencies.push_back(elapsed);
                }
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
    fn hedge_threshold(&self, class: RequestClass) -> Option<(Duration, Duration)> {
        let latencies = self.response_latencies.get(&class)?;
        if latencies.len() < MIN_HEDGE_SAMPLES {
            return None;
        }
        let mut sorted: Vec<_> = latencies.iter().copied().collect();
        sorted.sort_unstable();
        let rank = (sorted.len() * 4).div_ceil(5).saturating_sub(1);
        let p80 = sorted[rank];
        let threshold = p80
            .saturating_mul(HEDGE_LATENCY_MULTIPLIER)
            .saturating_add(HEDGE_LATENCY_OFFSET);
        Some((p80, threshold))
    }
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ModelTraceContext {
        ModelTraceContext {
            workflow: ModelWorkflow::Restoration,
            work_item_index: 0,
            conversation_turn: 0,
            request_kind: ModelRequestKind::Initial,
        }
    }

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
            None,
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
                None,
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
                None,
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
        assert_eq!(snapshot.max_concurrency, 3);
    }

    #[test]
    fn token_budget_counts_input_and_output_and_stops_at_the_limit() {
        let scheduler =
            scheduler(false, 2).with_token_budget(NonZeroU64::new(100).expect("non-zero budget"));

        scheduler.record_token_usage(Some(70), Some(29));
        assert_eq!(scheduler.check_token_budget(), Ok(()));
        scheduler.record_token_usage(Some(1), Some(0));

        assert_eq!(
            scheduler.check_token_budget(),
            Err(TokenBudgetError::Reached {
                limit: 100,
                consumed: 100,
            })
        );
        assert_eq!(
            scheduler.snapshot().token_budget,
            Some(TokenBudgetSnapshot {
                limit: 100,
                consumed: 100,
                missing_usage_responses: 0,
            })
        );
    }

    #[test]
    fn token_budget_fails_closed_when_usage_is_missing() {
        let scheduler =
            scheduler(false, 2).with_token_budget(NonZeroU64::new(100).expect("non-zero budget"));

        scheduler.record_token_usage(Some(40), None);

        assert_eq!(
            scheduler.check_token_budget(),
            Err(TokenBudgetError::UsageUnavailable {
                limit: 100,
                consumed: 0,
                missing_responses: 1,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn hedging_starts_only_after_ten_same_class_successes() {
        let scheduler = RequestScheduler::fixed(NonZeroUsize::new(20).unwrap(), Duration::ZERO);
        for seconds in 1..=10 {
            let permit = scheduler.acquire_for(context()).await;
            assert!(permit.hedge_plan().is_none());
            tokio::time::advance(Duration::from_secs(seconds)).await;
            permit.finish(RequestFeedback::Success);
        }

        let permit = scheduler.acquire_for(context()).await;
        let plan = permit.hedge_plan().expect("ten samples enable hedging");
        assert_eq!(plan.p80, Duration::from_secs(8));
        assert_eq!(plan.threshold, Duration::from_secs(29));
        permit.finish(RequestFeedback::Success);
    }

    #[tokio::test(start_paused = true)]
    async fn latency_samples_do_not_cross_request_classes() {
        let scheduler = RequestScheduler::fixed(NonZeroUsize::new(20).unwrap(), Duration::ZERO);
        for _ in 0..10 {
            let permit = scheduler.acquire_for(context()).await;
            tokio::time::advance(Duration::from_secs(1)).await;
            permit.finish(RequestFeedback::Success);
        }

        let mut repair = context();
        repair.request_kind = ModelRequestKind::Repair;
        let permit = scheduler.acquire_for(repair).await;
        assert!(permit.hedge_plan().is_none());
        permit.finish(RequestFeedback::Success);
    }

    #[tokio::test(start_paused = true)]
    async fn no_more_than_three_hedges_run_concurrently() {
        let scheduler = RequestScheduler::fixed(NonZeroUsize::new(20).unwrap(), Duration::ZERO);
        let class = RequestClass::from(context());
        scheduler
            .lock()
            .response_latencies
            .insert(class, VecDeque::from([Duration::from_secs(1); 10]));

        let mut tasks = Vec::new();
        for _ in 0..4 {
            let scheduler = scheduler.clone();
            tasks.push(tokio::spawn(async move {
                let _: Result<(), ()> = scheduler
                    .run_with_hedge(context(), |permit| async move {
                        let _permit = permit;
                        std::future::pending().await
                    })
                    .await;
            }));
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(8)).await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        let snapshot = scheduler.snapshot();
        assert_eq!(snapshot.active_hedges, 3);
        assert_eq!(snapshot.hedges_started, 3);

        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        assert_eq!(scheduler.snapshot().active_hedges, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_twentieth_hedge_exhausts_the_run_budget() {
        let scheduler = RequestScheduler::fixed(NonZeroUsize::new(20).unwrap(), Duration::ZERO);
        let class = RequestClass::from(context());
        scheduler.lock().hedges_started = 19;
        let plan = HedgePlan {
            scheduler: scheduler.clone(),
            class,
            logical_request_id: 1,
            threshold: Duration::from_secs(1),
            p80: Duration::ZERO,
        };
        let acquisition = tokio::spawn(async move { plan.acquire_after_threshold().await });
        tokio::time::advance(Duration::from_secs(1)).await;
        let permit = acquisition
            .await
            .unwrap()
            .expect("the twentieth hedge starts");
        assert_eq!(permit.metadata().hedge_number, Some(20));
        drop(permit);

        let exhausted = HedgePlan {
            scheduler: scheduler.clone(),
            class,
            logical_request_id: 2,
            threshold: Duration::ZERO,
            p80: Duration::ZERO,
        }
        .acquire_after_threshold()
        .await;
        assert!(exhausted.is_none());
        assert_eq!(scheduler.snapshot().hedges_started, 20);
    }

    #[tokio::test(start_paused = true)]
    async fn resumed_history_restores_samples_budget_and_logical_ids() {
        let mut records = Vec::new();
        for event_index in 0..10 {
            records.push(ModelTraceRecord {
                format_version: crate::MODEL_TRACE_FORMAT_VERSION,
                event_index,
                timestamp_unix_ms: 0,
                exchange_id: event_index,
                workflow: ModelWorkflow::Restoration,
                window_index: event_index as usize,
                conversation_turn: 0,
                request_kind: ModelRequestKind::Initial,
                event: ModelTraceEvent::Response {
                    provider_attempt: 0,
                    elapsed_ms: 1_000,
                    response: serde_json::json!({}),
                    raw_response: None,
                },
            });
        }
        records.push(ModelTraceRecord {
            format_version: crate::MODEL_TRACE_FORMAT_VERSION,
            event_index: 10,
            timestamp_unix_ms: 0,
            exchange_id: 10,
            workflow: ModelWorkflow::Restoration,
            window_index: 10,
            conversation_turn: 0,
            request_kind: ModelRequestKind::Initial,
            event: ModelTraceEvent::Request {
                provider_attempt: 0,
                endpoint: "test".into(),
                model: "test".into(),
                request: serde_json::json!({}),
                options: serde_json::json!({}),
                logical_request_id: Some(41),
                hedge: Some(crate::ModelHedgeMetadata {
                    logical_request_id: 41,
                    hedge_number: 1,
                    threshold_ms: 8_000,
                    p80_ms: 1_000,
                }),
            },
        });

        let scheduler = RequestScheduler::fixed(NonZeroUsize::new(20).unwrap(), Duration::ZERO);
        scheduler.restore_history(&records);
        assert_eq!(scheduler.snapshot().hedges_started, 1);
        let permit = scheduler.acquire_for(context()).await;
        assert_eq!(permit.metadata().logical_request_id, 42);
        assert_eq!(
            permit.hedge_plan().unwrap().threshold,
            Duration::from_secs(8)
        );
        permit.finish(RequestFeedback::Success);
    }

    #[test]
    fn resumed_history_contributes_to_the_token_budget() {
        let records = [ModelTraceRecord {
            format_version: crate::MODEL_TRACE_FORMAT_VERSION,
            event_index: 0,
            timestamp_unix_ms: 0,
            exchange_id: 0,
            workflow: ModelWorkflow::Restoration,
            window_index: 0,
            conversation_turn: 0,
            request_kind: ModelRequestKind::Initial,
            event: ModelTraceEvent::Response {
                provider_attempt: 0,
                elapsed_ms: 1_000,
                response: serde_json::json!({
                    "usage": {"prompt_tokens": 80, "completion_tokens": 20}
                }),
                raw_response: None,
            },
        }];
        let scheduler =
            scheduler(false, 2).with_token_budget(NonZeroU64::new(100).expect("non-zero budget"));

        scheduler.restore_history(&records);

        assert!(matches!(
            scheduler.check_token_budget(),
            Err(TokenBudgetError::Reached {
                limit: 100,
                consumed: 100
            })
        ));
    }
}
