# Provider rate-limit contracts and automatic concurrency

Research checked against official documentation on 2026-09-06. This is a research note, not an architectural decision. No account credentials or paid API requests were used.

## What the provider contracts establish

The reviewed contracts do not establish a universal API for discovering the best concurrency. They expose different combinations of configured quotas, remaining capacity, retry advice, and account-level settings. A concurrency cap bounds simultaneous work; requests-per-minute and tokens-per-minute constrain consumption over time. These are separate controls. The comparison below supports that distinction; an optimal client policy is an engineering inference, not a provider guarantee.

| Provider | Online limits and scope | Token accounting | Discovery and feedback |
| --- | --- | --- | --- |
| OpenAI | RPM/TPM and additional daily/modality limits; organization and project scope; models can share limit pools. | The rate-limit guide describes charging the greater of `max_tokens` and a character-based input-token estimate. Verify endpoint/model details before treating this as a universal input-plus-output reservation. | Account limits page; `x-ratelimit-*` request/token limit, remaining and reset headers; optional project-token headers. `Retry-After`, when present, gives a minimum wait. Rapid traffic growth can cause `slow_down` despite staying below RPM/TPM. [Rate limits](https://developers.openai.com/api/docs/guides/rate-limits). |
| Anthropic | Separate RPM, input TPM and output TPM per model class, with organization and optional workspace limits; acceleration limits also apply. | Input tokens are estimated then adjusted. Most models exclude cached reads from input TPM. Output TPM counts actual generated tokens in real time; `max_tokens` does **not** reserve output TPM. | Console and documented Rate Limits API; `anthropic-ratelimit-*` limit/remaining/reset headers for requests and input/output tokens; reset timestamps use RFC 3339. `retry-after` gives seconds. A spend-cap 429 can lack this header and require account action. [Rate limits](https://platform.claude.com/docs/en/api/rate-limits). |
| Gemini Developer API | Usually RPM, input TPM and requests/day, per project rather than API key; limits vary by model and tier. Some accounts also have rolling spend-based limits. | The documented TPM dimension is input tokens. Do not import another provider's output-reservation rule. | Active limits appear in AI Studio. The reviewed page does not specify a general remaining/reset response-header contract or a general quota-discovery endpoint. This is a documentation limitation, not proof no other interface exists. Published limits do not guarantee available capacity. [Rate limits](https://ai.google.dev/gemini-api/docs/rate-limits). |
| Azure OpenAI | Subscription quota is divided by region, model and deployment type, then allocated to deployments. RPM follows a model-dependent ratio to allocated TPM. Short enforcement windows can throttle bursts below minute totals. | Admission uses an estimated maximum processed-token count, including prompt, `max_tokens` and applicable `best_of`, rather than final billed tokens. | Portal and Azure Resource Manager quota/capacity APIs; request/token limit, remaining and reset headers; `retry-after-ms` on 429. Shared capacity can temporarily lower effective limits. Management quota allocation does not represent current inference headroom. [Manage quota](https://learn.microsoft.com/en-us/azure/foundry-classic/openai/how-to/quota). |

OpenAI also documents `GET /organization/projects/{project_id}/rate_limits`, returning per-model configured request/token limits. Its example uses an admin API key, so a client holding only ordinary inference credentials must not assume this discovery capability is available. [List project rate limits](https://developers.openai.com/api/reference/ruby/resources/admin/subresources/organization/subresources/projects/subresources/rate_limits/methods/list_rate_limits).

## Provider Batch APIs are a separate execution choice

Sending many ordinary online requests concurrently retains the online quota contract. OpenAI's Batch API instead accepts asynchronous jobs, uses a separate rate-limit pool, and has a 24-hour completion window. It has per-batch, queued-prompt-token and batch-creation limits; expired batches can contain unfinished requests. This suits work that tolerates delayed results rather than providing an automatic concurrency setting for an interactive workflow. [Batch API](https://developers.openai.com/api/docs/guides/batch).

Anthropic likewise documents a separate Message Batches RPM limit and processing-queue capacity shared across models. [Message Batches limits](https://platform.claude.com/docs/en/api/rate-limits#message-batches-api). Gemini documents separate batch limits, including concurrent batch requests and enqueued tokens per model. Those batch-concurrency limits must not be interpreted as online-request concurrency limits. [Gemini batch limits](https://ai.google.dev/gemini-api/docs/rate-limits#batch-api-rate-limits).

## Immediate implementation implications

- Normalize provider signals through adapters; preserve the raw response headers and structured error code.
- Scope shared accounting to the actual quota pool. Multiple keys, workflows or models can consume one pool.
- Treat quota discovery as optional. Configuration, response feedback and observed throttling must remain useful when administrative APIs are unavailable.
- Distinguish temporary throttling from exhausted billing/spend limits. A retry loop cannot repair the latter.
- Make token estimates provider-specific; current Anthropic output accounting explicitly differs from Azure's admission estimate.

These are recommendations inferred from the contracts above. They do not establish one universally optimal concurrency algorithm.

## Established scheduling implementations

The common pattern is a queue with separate controls for request rate, token
consumption, and concurrent HTTP requests. The implementations below support
that conclusion; none establishes a universal optimum for arbitrary LLM endpoints.

- OpenAI's reference parallel processor replenishes separate request and token
  budgets, dispatches only when both have capacity, and pauses sending after
  rate-limit errors. Its budgets are supplied by the caller, not discovered
  automatically. This is an illustrative implementation, not a universal SDK.
  [Reference source](https://github.com/openai/openai-cookbook/blob/main/examples/api_request_parallel_processor.py)
- AWS SDK adaptive mode adjusts a client's sending rate using throttling
  feedback, including delaying initial requests. It is an opt-in strategy for
  suitable workloads, not the recommended default for every application. All
  requests on one client share that controller, so unrelated quota scopes should
  not be coupled. Standard retries also use backoff with jitter and a retry
  budget.
  [AWS retry behavior](https://docs.aws.amazon.com/sdkref/latest/guide/feature-retry-behavior.html)
- Netflix's concurrency-limits library offers client-side congestion control,
  including additive increase and multiplicative decrease (AIMD). Its AIMD
  implementation raises the limit under sufficient utilization and reduces it
  on a dropped request or excessive latency, within configured bounds. This
  provides an established algorithm, not LLM quota accounting.
  [Project rationale](https://github.com/Netflix/concurrency-limits/blob/main/README.md),
  [AIMD implementation](https://github.com/Netflix/concurrency-limits/blob/main/concurrency-limits-core/src/main/java/com/netflix/concurrency/limits/limit/AIMDLimit.java)
- Envoy's adaptive concurrency filter compares observed latency with a sampled
  baseline. Its documentation explicitly limits the conditions under which the
  controller can work reliably, including controlling the relevant traffic.
  [Envoy adaptive concurrency](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/adaptive_concurrency_filter)
- LiteLLM's router exposes deployment RPM/TPM settings, parallel-request limits,
  retries, and cooldowns. This is useful evidence of established LLM scheduling
  controls; it does not promise automatic discovery of every provider's limits.
  Running its gateway would be a separate deployment decision for BeyondSlides.
  [LiteLLM routing](https://docs.litellm.ai/docs/routing)

An HTTP 429 can include Retry-After, but HTTP does not specify the server's
counting scope or require quota discovery. Retry-After can contain either
delay seconds or an HTTP date.
[RFC 6585 section 4](https://www.rfc-editor.org/rfc/rfc6585.html#section-4),
[RFC 9110 section 10.2.3](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3)

## Recommendation for BeyondSlides

The following is a proposed design inferred from the sources, not a standard,
an implemented feature, or a measured optimum.

1. Schedule each actual model HTTP attempt through one shared controller for
   its quota scope. Initial requests, tool follow-ups, structured-answer repairs,
   and provider retries all consume capacity. Separate workflow/session
   concurrency from HTTP concurrency. Release the HTTP permit while waiting
   for retry backoff or executing local tools.
2. Accept optional known request/token quotas and a configurable concurrency
   ceiling. Start work when both quota budgets and an in-flight slot permit it.
   Provider-specific accounting belongs in adapters: input/output budgets,
   output reservations, cached tokens, reset formats, and quota scope vary.
   Missing token usage stays unknown; an estimate must be labelled as such.
3. For opaque endpoints, start with bounded load and adapt the sending rate
   using server feedback. Increase gradually while useful throughput improves;
   on throttling, share a cooldown across that quota scope and reduce sending
   pressure. Add jitter after any mandatory Retry-After delay and bound retries.
   A 429 caused by exhausted credit or a daily cap should not trigger endless
   throughput probing. Automatic tuning needs configured bounds and cannot
   guarantee no initial throttling.
4. If adding adaptive concurrency, use a small AIMD-style controller and
   measure successful work per wall-clock minute. Do not increase concurrency
   just because a request succeeded: there must be queued work and evidence the
   current cap is limiting throughput. Do not increase it while already blocked
   on RPM/TPM. Avoid treating raw latency across restoration, tool calls, and
   larger novelty requests as one stationary signal.
5. Keep timeout diagnosis separate. The recorded novelty canary failed while
   it was the only comparison in flight. A smaller concurrency cap cannot solve
   a request that independently exceeds a gateway deadline. Smaller comparison
   batches, output/reasoning settings, or a different transport/endpoint require
   their own evaluation; streaming is not a guaranteed workaround for an
   arbitrary proxy's timeout policy.
6. Record effective concurrency, request/token budget waits, cooldown waits,
   provider latency, retries, and accepted work per minute. Retain checkpoints.
   Separate operational pacing/concurrency settings from semantic checkpoint
   identity so a scheduler adjustment does not require redoing accepted work.
   Continue validating inputs, model settings, prompts, and outputs for reuse.

For a first implementation, prioritize shared admission, configurable ceilings,
known quotas when available, and shared feedback/backoff for unknown limits.
Automatic concurrency adjustment can build on those measurements. A separate
maximum-load preflight should not be required for every lecture: useful
requests can supply the feedback during the run.

Quota state should match the provider's actual resource boundaries, which can
span keys, models, deployments, or projects. Default grouping by configured
endpoint and credential identity is only an approximation for opaque proxies;
allow explicit grouping and never persist the credential itself. A single CLI
can coordinate its own work but cannot know traffic from other programs unless
the provider reports it or a shared scheduler is introduced.

## Reusable Rust pieces and current integration gaps

Tokio's Semaphore provides in-flight permits; governor provides asynchronous
rate limiting, weighted acquisition, and optional waiting jitter. Tower has
configured concurrency/rate middleware. These are reusable mechanisms, not a
complete adaptive LLM scheduler; none automatically understands every provider's
token quota semantics.
[Tokio Semaphore](https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html),
[governor RateLimiter](https://docs.rs/governor/latest/governor/struct.RateLimiter.html),
[Tower concurrency limit](https://docs.rs/tower/latest/tower/limit/concurrency/struct.ConcurrencyLimitLayer.html)

Local source inspection on 2026-09-06 found:

- The analysis CLI sets four concurrent windows and a shared five-second
  interval, independently of the configured endpoint.
  [analysis_run.rs](../../src/analysis_run.rs)
- Each failed request sleeps independently. Retry-After parsing accepts only
  integer seconds and clips the delay to 120 seconds. The fallback has no jitter.
  It neither establishes a shared cooldown nor learns sustainable capacity.
  A future scheduler should honor a longer server delay or defer/fail clearly,
  rather than retrying earlier by clipping it.
  [chat_completions.rs](../../src/chat_completions.rs)
- Cargo.lock pins genai 0.6.5. Its local ChatResponse definition has no successful
  response-header field; WebClient's success path returns status and body after
  discarding headers. Failure responses do retain headers. Header-driven
  scheduling will require an appropriate transport hook or library enhancement,
  and must degrade gracefully where headers are absent.
  [Cargo.lock](../../Cargo.lock); inspected installed crate files
  `src/chat/chat_response.rs` and `src/webc/web_client.rs`.
- AnalysisRunManifest currently compares concurrency and pacing alongside
  semantic inputs. Changing those settings therefore invalidates resume today.
  The proposed separation above is a future change, not permission to edit old
  manifests or assume old artifacts match.
  [analysis_run.rs](../../src/analysis_run.rs)

This research changed documentation only. No traffic probe, paid model request,
production code change, or interruption of the active lecture run was performed.
