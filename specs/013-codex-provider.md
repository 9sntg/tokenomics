# Spec 013 — Codex provider (OpenAI subscription account)

Status: **Done**

## Motivation

The provider seam (`ProviderAdapter`, spec 002) was designed so Codex/Gemini/Grok are additive.
The user now has a working Linux `codex` CLI (0.144.1, linuxbrew) logged into a ChatGPT **team**
subscription, with real data on this machine — so the first non-Claude adapter lands. Verified live
(2026-07-10):

- **Usage (local plane, ToS-safe):** `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` carries
  `event_msg` / `token_count` events with `info.total_token_usage` (cumulative per session) and
  `info.last_token_usage` (per-turn delta): `input_tokens`, `cached_input_tokens`, `output_tokens`,
  `reasoning_output_tokens`, `total_tokens`, each event timestamped.
- **Limits (authoritative plane, opt-in):** the rollout files' `rate_limits` field is `null` in
  practice (upstream issue #14728), but `codex app-server` JSON-RPC over stdio answers
  `account/rateLimits/read` with `primary` (5h: `windowDurationMins: 300`) and `secondary` (weekly:
  `windowDurationMins: 10080`), each `{usedPercent, resetsAt}` (`resetsAt` = epoch seconds), plus
  `planType`. This is the Codex equivalent of Claude's `/api/oauth/usage` overlay: real server
  values for the account whose `CODEX_HOME` we run under.

## Behaviour

### A. Provider enum + config

- `Provider::Codex` joins the enum (`"codex"` in config, store, display). Adding the variant fixes
  every exhaustive match crate-wide (the compiler flags them; that is the seam working as designed).
- A Codex account is one `[[account]]` block with `provider = "codex"` and `config_dir` = that
  account's **`CODEX_HOME`** (default `~/.codex`). Attribution is the `config_dir`, never the logs —
  same invariant as Claude, different env var.
- `limits_overlay = true` on a Codex account opts into the app-server limits fetch (network — same
  opt-in posture as the Claude overlay). Default off.

### B. Usage plane — sessions JSONL (local, no network)

- New pure core `providers/codex/sessions.rs` (mirrors `claude/ccusage.rs` in role):
  - `parse_rollout_events(bytes) -> Vec<TokenCountEvent>`: extract `token_count` events (timestamp +
    `last_token_usage` buckets) from one rollout file; unknown/other lines skipped defensively
    (malformed lines never fail the file; a malformed *file* yields the events parsed so far).
  - `reduce_codex_snapshot(events, account_id, now) -> Option<UsageSnapshot>`: sum the
    `last_token_usage` deltas of events with `timestamp ≥ now − 5h`. Bucket mapping:
    `input = input_tokens − cached_input_tokens` (floor 0), `cache_read = cached_input_tokens`,
    `output = output_tokens` (reasoning tokens are already inside it), `cache_creation = 0`,
    `total_tokens` = sum of delta `total_tokens`. No in-window events ⇒ `None` (idle — the
    `ProviderAdapter` contract).
  - `cost_notional = None` (no public subscription pricing basis — we never invent a proxy) and
    `window = None` (Codex exposes no local block; the 5h lookback is a scan bound, not a window
    claim). Consequently **no derived session limit exists for Codex** — without the overlay the
    session gauge is honestly `n/a`, never a guess.
- `CodexAdapter` (in `providers/codex/mod.rs`) implements `ProviderAdapter::collect`: enumerate
  `<config_dir>/sessions/**/rollout-*.jsonl` with file mtime within the lookback (cheap prune; the
  date-tree walk stays bounded to the covering dates), parse + reduce. Missing `sessions/` dir ⇒
  idle (`Ok(None)`), not an error — a fresh install shows an empty account, not a red row.

### C. Limits plane — `codex app-server` RPC (opt-in, authoritative)

- New `providers/codex/rate_limits.rs`:
  - Pure `parse_rate_limits_response(lines, account_id, warn, crit) -> AppResult<Vec<Limit>>` (no
    injected `now` — the epoch→RFC 3339 conversion is absolute): find the
    JSON-RPC response whose `id` matches the rateLimits request; map `primary` → `LimitKind::Session`,
    `secondary` → `LimitKind::WeeklyAll` (no scoped weeklies — Codex has no per-model data);
    `usedPercent` → `utilization_pct`; `resetsAt` (epoch seconds) → RFC 3339 UTC string in
    `resets_at` (one normalization at the source boundary; rendered verbatim from there on);
    severity via `format::severity_for`; provenance **`Authoritative`**. Missing/malformed body ⇒
    error (the caller degrades, never invents).
  - A subprocess client behind an injectable seam (same testing posture as `Runner`/`UsageEndpoint`):
    spawn argv `["codex", "app-server"]` with `CODEX_HOME` pinned to the account's `config_dir`,
    write the `initialize` then `account/rateLimits/read` request lines, **hold stdin open**, read
    stdout until the matching response id (the child exits/killed after; `kill_on_drop`), all under
    one hard timeout. Never a shell string; no secret is read, logged, or stored (auth stays inside
    the codex binary).
- Collector integration: opted-in Codex accounts join the existing **overlay cadence** pass
  (`poll_overlay_secs`, budget, 429/failure backoff, TTL demotion to `Estimate` — all reused).
  Claude-specific gates (credentials-file warmth, "token stale — open Claude") do **not** apply to
  Codex: eligibility is just `active && limits_overlay`, and any failure (logged out, binary
  missing, timeout) degrades silently per the architecture and is diagnosable via `doctor`.

### D. Surfaces

- `tok once [--json]` prints Codex accounts like any other (tokens; no cost line — `cost_notional`
  is `None`; no derived session %).
- `tok doctor` for a Codex account: `config_dir` exists, `sessions/` present, `codex --version`
  runs, `auth.json` present (existence only — content never read); opted-in accounts get an
  app-server reachability check mirroring the Claude overlay check.
- TUI: a Codex account renders with the same row grammar — session + weekly gauges when the overlay
  delivers, `n/a` hints otherwise. The fleet header keeps its existing **representative (max)**
  reduction — designed for the shared-`projects/` Claude lane, where every account reads the same
  stream, so summing would multiply-count; a Codex account's (much smaller) stream therefore doesn't
  add to the headline number, it just can't distort it. `cost_notional = None` must not poison the
  fleet cost reduction (reduce over present values only). Known ceiling: a heterogeneous fleet
  under-reports total tokens by design until the reduction learns per-stream grouping (future wave).

## Non-goals

- Parsing rollout `rate_limits` (null in practice — #14728) or the Codex sqlite databases.
- Cost estimation for Codex (no honest basis).
- Multi-Codex-account UX beyond what `CODEX_HOME` isolation already gives (unverified upstream;
  one account configured today).
- Auto-login / token refresh for Codex (same posture as Claude: we never touch auth).
- Weekly-scoped (per-model) limits for Codex — the endpoint has none.

## Acceptance criteria

1. `provider = "codex"` parses; `Provider::parse("codex")` round-trips; config validate accepts a
   Codex account. (A)
2. `parse_rollout_events` extracts token_count events from real-shape fixture bytes; skips
   malformed lines; `reduce_codex_snapshot` sums only in-window deltas with the documented bucket
   mapping; no in-window events ⇒ `None`. (B)
3. `CodexAdapter::collect` on a fixture sessions tree returns the reduced snapshot; missing
   `sessions/` ⇒ `Ok(None)`. (B)
4. `parse_rate_limits_response` maps primary/secondary → Session/WeeklyAll with `usedPercent`,
   RFC 3339 `resets_at`, `Authoritative` provenance; malformed ⇒ error. (C)
5. The app-server client is exercised through its seam with a canned transcript (no process spawn
   in unit tests); the real client pins `CODEX_HOME`, holds one timeout, and is argv-only. (C)
6. Collector: an opted-in Codex account gets authoritative limits on the overlay pass; a failing
   fetch backs off and the TTL demotion applies unchanged; a Claude account's path is untouched
   (existing collector tests stay green). (C)
7. `tok once --json` includes the Codex account; `doctor` reports the Codex checks. (D)
8. A TUI snapshot covering a Codex account renders gauges from authoritative limits and `n/a`
   hints without a derived session guess. (D)
9. `./check.sh` green.
