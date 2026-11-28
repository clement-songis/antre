# Rate Limits and Quotas

Providers stop answering for two different reasons, and Moltis treats them
differently:

- **Rate limits**: too many requests or tokens per minute. Waiting seconds to
  a few minutes fixes them.
- **Quotas**: a plan or subscription allowance is used up (Claude or Codex
  five-hour and weekly windows, the GLM coding plan, monthly credits). Waiting
  until the reset fixes them.

Billing problems (no credit left) and authentication errors are neither:
waiting does not help, so they fail immediately with a clear message.

## How errors are recognised

Each provider turns a failed HTTP response into a typed error from its
status, headers and body:

| Provider | Where the reset comes from |
|---|---|
| Any | `Retry-After` (seconds or HTTP date) |
| Anthropic API keys | `anthropic-ratelimit-*-reset` (RFC 3339, the exhausted bucket) |
| Claude subscription | `anthropic-ratelimit-unified-reset` and the window in `...-representative-claim` |
| OpenAI | `x-ratelimit-reset-requests` / `-tokens` (durations such as `6m0s`) |
| Codex subscription | `x-codex-{primary,secondary}-reset-after-seconds` and window minutes, or `resets_at` / `resets_in_seconds` in the body |
| Z.AI / GLM coding plan | error codes 1308 / 1310 with a time such as `2026-11-28 03:15:42` in the message |

Times printed without an offset are read in the provider's timezone from the
model catalog (`rate_limit_tz`; China Standard Time, UTC+8, for Z.AI), never
in the server's local zone. Every instant is stored in UTC. A parsed reset in
the past or more than eight days ahead is treated as unknown.

## What happens

1. **A fallback exists** (`failover.fallback_models` or other chain entries):
   the run moves on to the next provider, and the exhausted one is skipped
   until its reset instead of for a fixed minute.
2. **Short rate limits** (a minute or less): the run retries in place with
   backoff.
3. **Every provider is out and the reset is known** within
   `chat.quota_wait.max_wait_hours` (default 6 h): the run is parked. The
   chat shows "Waiting for quota" with a countdown and a Cancel button;
   channels (Telegram, Discord, Signal...) get a message saying when the run
   resumes, in your timezone. At the reset (plus up to 30 s of jitter) the run
   resumes with the same message, which is not stored twice.
4. **Reset unknown or too far away**: the run fails with the provider's
   message and, when known, the reset time.

Parked runs are saved in `data_dir()/chat/quota-waits.json` and rescheduled
after a restart; a reset that passed while Moltis was down resumes right away.
Cancel from the chat card, or with the `chat.cancel_quota_wait` RPC
(`{"sessionKey": "..."}`).

A parked run keeps its session: messages sent while it waits are queued behind
it, and they run after it resumes, or right away if you cancel the wait. See
[Reliable delivery and session lanes](delivery.md#one-run-per-session).

```toml
[chat.quota_wait]
enabled = true
max_wait_hours = 6
```
