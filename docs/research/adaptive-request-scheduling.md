# Adaptive admission for online model requests

Primary sources checked on 2026-09-06. This note proposes a small controller for
BeyondSlides; it does not establish a provider-independent optimum or report a
live throughput measurement. It supplements
[provider-rate-limit-contracts.md](provider-rate-limit-contracts.md). No paid API
requests were made for this research.

## Recommendation

Use one shared admission controller for actual HTTP attempts within a configured
quota scope. Combine a bounded adaptive concurrency cap with a paced start time
and shared cooldown. Use explicit throttling feedback for the first adaptive
policy, with gradual recovery after healthy, utilized periods. Preserve an
explicit hard maximum and a fixed mode. Treat latency as diagnostic data until
measurements justify a more complicated latency controller.

This is an engineering recommendation inferred from the sources below. It is
compatible with [ADR 0005](../adr/0005-rank-importance-and-novelty-comparatively.md):
it changes how comparative-judgment HTTP attempts are admitted, not passage
preparation, comparative groups, scoring, or checkpoint semantic identity.

## What established algorithms offer

| Approach | Primary-source behavior | Implication for BeyondSlides |
| --- | --- | --- |
| AIMD | Netflix increases the concurrency limit under sufficient utilization and multiplicatively reduces it after a drop or configured latency timeout, bounded by minimum and maximum limits. | Small state and explicit feedback make this a useful starting point. Its defaults and per-sample increase rule are not calibrated for long LLM requests. |
| Vegas | Netflix estimates queued work as `limit * (1 - no_load_rtt / actual_rtt)`, changes the limit around queue thresholds, and suppresses growth when application demand is low. | A longer answer can resemble server queueing even when the endpoint is healthy. |
| Gradient / Gradient2 | Envoy compares a sampled latency with a periodically measured baseline. Netflix Gradient2 instead compares current latency with a long-term smoothed baseline to reduce bias from heterogeneous request complexity and data size. | Smoothing helps but cannot identify whether changing task mix or server congestion caused slower responses. |

Sources: [Netflix AIMD source](https://github.com/Netflix/concurrency-limits/blob/main/concurrency-limits-core/src/main/java/com/netflix/concurrency/limits/limit/AIMDLimit.java),
[Netflix Vegas source](https://github.com/Netflix/concurrency-limits/blob/main/concurrency-limits-core/src/main/java/com/netflix/concurrency/limits/limit/VegasLimit.java),
[Netflix Gradient2 source](https://github.com/Netflix/concurrency-limits/blob/main/concurrency-limits-core/src/main/java/com/netflix/concurrency/limits/limit/Gradient2Limit.java).

Envoy periodically reduces concurrency to sample a baseline, and explicitly
requires control of the relevant upstream traffic for its controller to work as
intended. One CLI using a shared provider cannot assume it controls traffic from
other customers, account users, or programs. Its full algorithm is therefore not
a portable guarantee for this application.
[Envoy adaptive concurrency](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/adaptive_concurrency_filter).

The LLM-specific inference is that restoration, passage preparation, importance
comparisons, novelty comparisons, tool continuations, and answer repairs are
different service-time populations. Group operational measurements by request
kind and token size where known. An overall latency percentile may describe the
run, but should not automatically cause cap reductions. A request that exceeds a
gateway deadline while running alone needs separate request/endpoint diagnosis.

## Rate and concurrency are separate controls

A cap of one still permits 600 requests/minute if each request completes in
100 ms. It cannot enforce a hypothetical 60 RPM quota. Conversely, pacing starts
one second apart permits 30 overlapping requests when each takes 30 seconds.
Both controls are needed; neither example requires token accounting to show the
distinction. Actual token limits remain a separate provider-specific admission
dimension, as documented in the accompanying provider note.

AWS adaptive retry mode is useful precedent: a client-wide limiter adjusts the
send rate using throttling feedback, delaying initial attempts as well as
retries. AWS recommends it for appropriate single-resource workloads rather
than as a universal default, because one resource's throttling affects all
requests sharing that client. Standard retries separately provide jitter and a
bounded retry budget.
[AWS SDK retry behavior](https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html).

HTTP 429 means excessive requests over time; the specification leaves user
identification and counting scope to the server and makes Retry-After optional.
It cannot identify RPM versus TPM versus a concurrency quota by itself.
[RFC 6585 §4](https://www.rfc-editor.org/rfc/rfc6585.html#section-4).

## Proposed minimal policy

The following is a testable application policy, not a verbatim implementation of
Netflix, TCP, or AWS algorithms. Its tuning constants need experiment evidence.

1. Maintain `hard_max`, `effective_limit`, `in_flight`, `next_start`,
   `cooldown_until`, a pacing interval, and a feedback generation. Start the
   effective limit conservatively inside `1..=hard_max`. Apply any configured
   minimum interval as a floor. An automatic mode needs a positive bootstrap
   interval if its multiplicative pacing adjustment would otherwise start at
   zero.
2. Every actual HTTP attempt checks the same admission state: initial calls,
   provider retries, tool follow-ups, and answer repairs. Wait until the cooldown
   and paced start allow sending and `in_flight < effective_limit`. Known
   provider request/token budgets can add gates without changing the contract.
   Local tools and retry sleeps do not occupy HTTP slots.
3. On temporary throttling, extend the shared cooldown and reduce pressure:
   for example halve the effective cap, bounded at one, and double the pacing
   interval. These factors are candidate tuning values, not provider facts.
   Adaptation of pacing remains necessary when the cap is already one.
4. Capture the feedback generation on admission. The first throttling result
   from the current generation reduces pressure and advances the generation.
   Other failures from that older cohort still extend the cooldown and reset
   the healthy-period counter, but do not repeatedly halve the cap. Old
   successful completions cannot increase it. This avoids interpreting eight
   responses from one already-dispatched wave as eight independent probes.
5. Recover only after enough current-generation successful completions and a
   minimum elapsed healthy period, with queued work and evidence that admission
   is limiting demand. Increase the cap by at most one per period when slots
   were the limiting gate. Relax learned pacing gradually when pacing was the
   limiting gate. Do not grow the cap merely because a paced request succeeded,
   or simultaneously accelerate both controls on every completion. Keep known
   quota gates and the user's hard bounds authoritative.
6. Keep retries bounded independently of learning. Known billing, credit, or
   nonrecoverable quota errors should terminate the affected work. An unknown
   429 may be retried within bounds but must not be interpreted as proof that
   repeated retries will eventually succeed.

The cohort rule above is an application inference. TCP's congestion-avoidance
algorithm is useful precedent for limiting additive growth over a feedback
round, but HTTP requests are not TCP segments and this note does not claim TCP
conformance. [RFC 5681 §3.1](https://www.rfc-editor.org/rfc/rfc5681.html#section-3.1).

## Cooldown, error classes, and delayed responses

Retry-After accepts either nonnegative integral seconds or an HTTP date. A 503
can also include it; 503 can mean overload or maintenance, whereas 504 describes
a gateway waiting too long for its upstream.
[RFC 9110 §10.2.3](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3),
[RFC 9110 §15.6](https://www.rfc-editor.org/rfc/rfc9110.html#section-15.6).

Recommended handling:

- Convert a valid server delay to a monotonic deadline at receipt; extend
  shared state with `max(old_deadline, new_deadline)`. Add nonnegative jitter
  after the requested wait. Never shorten a valid long server delay to a local
  backoff ceiling; if the run cannot wait that long, defer or return an explicit
  error. Past dates yield no positive server delay; malformed values use the
  bounded fallback policy.
- Recheck shared state after every wake. A waiter sleeping until an earlier
  deadline must observe a subsequently extended cooldown before admission.
  At expiry, use paced starts rather than releasing all sleepers together.
- Retry eligible network failures and transient 500/502/503/504 responses with
  bounded jittered backoff. Honor relevant server delay advice. Do not learn a
  permanent RPM reduction from every 5xx. Explicit provider error codes may
  classify an otherwise ambiguous status as throttling; repeated overload can
  justify a separately measured availability policy.
- Suppress growth during an unhealthy period even when the failures do not
  establish a quota reduction. Cancellation is neither success nor throttling.

AWS likewise classifies structured errors before falling back to status codes;
for example, an explicit throttling error on a 5xx is treated differently from
an ordinary transient 5xx. That classification is useful precedent, while AWS's
specific error names and delay constants must not be imported into arbitrary
OpenAI-compatible endpoints.
[AWS error classification](https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html#retry-mode-implementation).

## Cancellation-safe Rust implementation seam

Keep policy state transitions synchronous and testable using supplied times;
wrap them in one asynchronous admission method returning an owned guard.
Atomically commit `in_flight += 1` and the next paced start only when sending is
allowed. Dropping the guard must release the slot exactly once, including
cancellation and early-return paths. Report feedback once, separately from
unconditional release. A cap decrease never cancels requests already running;
new admissions wait for the count to fall below the new cap.

Avoid reserving arbitrarily many future start times before waiting: canceled
waiters leave artificial gaps, and later cooldown changes invalidate those
reservations. Waiting must not hold the state mutex or an HTTP permit. If
waiter counts influence growth, cancellation must also remove that demand.

Tokio permits release on drop and semaphore acquisition is fair, although
cancellation loses queue position. These are useful building blocks for a fixed
ceiling; adaptive shrinking is easier to reason about as an admission predicate
than by having a task asynchronously acquire permits to remove them.
[Tokio Semaphore](https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html).

For a mutex plus Notify design, establish notification interest before testing
the predicate, then loop after waking. Tokio documents the lost-wakeup hazard
with multiple waiters and `notify_one`; `Notified::enable` is its supplied
solution. `notify_waiters` wakes already-created notification futures, so the
future must exist before the state check. A notification never substitutes for
rechecking the predicate.
[Tokio Notify](https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html).

Governor provides asynchronous rate waiting, weighted cells, and jitter. It
does not by itself supply this feedback policy or provider token semantics.
For the first controller, existing Tokio primitives plus a small policy avoid
adding a dependency solely for adaptive behavior; governor becomes useful if
known weighted budgets are implemented.
[Governor RateLimiter](https://docs.rs/governor/latest/governor/struct.RateLimiter.html).

## Verification that distinguishes useful behavior

Use a deterministic clock and scripted/local responses to cover: hard-cap
enforcement; no admission before a shared cooldown; a cooldown extended while
another request sleeps; paced release after expiry; cap-one rate throttling;
one reduction for a correlated cohort; no growth from stale successes; healthy
recovery with demand; no growth without demand; cancellation of queued and
admitted requests; transient 5xx handling; long and malformed Retry-After; and
retries/tool continuations passing through the same gate.

An experiment should report accepted useful work per elapsed minute, attempts,
retries, 429/5xx counts, cap changes, admission wait, and request latency by kind.
Use useful requests to learn within configured bounds. Finite runs, changing
token sizes, and unobserved shared traffic prevent an honest promise that this
discovers maximum throughput. Keep semantic output validation and checkpoint
reuse independent of scheduler tuning.
