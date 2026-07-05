# Spec 011 — Refresh & freshness hardening

Status: **Done** (all acceptance criteria pass; `./check.sh` green)

## Motivation

The board can display **stale data as if it were current**, and the refresh path degrades in ways the
UI never surfaces. An adversarial review of the collector → store → TUI refresh path (14 confirmed
findings, verified against source; corroborated by ratatui / tokio / SQLite / Grafana best-practice
research) found six root causes behind the reported "it refreshes slower / is it actually current?":

- **A. Collector liveness is invisible.** `store.heartbeat("collector", …)` is written every local
  tick (`collector.rs`) but **read by no production code** — the `heartbeat` table is write-only dead
  code. If the collector was never started, crashed, was OOM-killed, or hung, the TUI keeps redrawing
  the last-good rows forever and looks perfectly healthy.
- **B. The local ccusage plane has no freshness signal.** The only "refreshed Nm ago" label is fed
  **exclusively** by the overlay's `last_overlay_success` (`build_fleet_view`). For an opted-**out**
  account — the documented default, the overlay is opt-in — there is *no* time reference at all, and
  even opted-in, a fresh overlay success makes the whole line read "refreshed just now" while the
  local tokens/cost/burn beside it are frozen.
- **C. Stale authoritative limits never degrade to derived.** On a stale token or persistent 429 the
  collector calls `mark_stale` and `continue` but **never clears the stored authoritative limits**.
  Because `merge_limits` ranks `Authoritative > Derived`, the frozen authoritative %/reset keeps
  winning the merge indefinitely — the derived fallback the architecture promises never engages. This
  violates the CLAUDE.md invariant "degrade silently to derived estimates on any 429/failure" and can
  **hide a real crit** behind a stale-but-calm authoritative number (alerts key off it).
- **D. The overlay pass is awaited inline and runs sequentially.** `overlay_tick` is `.await`-ed
  inside the collector's `select!` for up to `OVERLAY_TICK_BUDGET_SECS` (20s), so every overlay cycle
  the **local plane freezes** (no new ccusage collect, finished collects not persisted) for seconds.
  Within the pass, accounts are fetched one-by-one under the budget, so on a slow network only
  ~`budget / timeout` ≈ 2 of N accounts actually refresh per pass.
- **E. The header aggregate is an unbounded scan with no retention.** `aggregate_burn_history` runs a
  full-table `GROUP BY` over the ever-growing `snapshots` table **every 1s**, and nothing prunes the
  table or checkpoints the WAL. Refresh genuinely slows the longer the collector runs, and the `.db`
  / `-wal` grow without bound.
- **F. No `MissedTickBehavior`.** Both collector intervals (and the TUI tick) default to `Burst`, so a
  WSL suspend/resume replays the whole backlog of missed ticks as a spike.

Two review findings were **refuted** and are deliberately *not* addressed: clamping the ccusage child
timeout below the poll interval (regresses a slow-but-succeeding account into a permanent freeze), and
moving reads to `spawn_blocking` (WAL + single-writer already precludes the claimed read-side stall).

## Principles carried in

- **Two planes, never conflated** (CLAUDE.md). Freshness is per-plane: the local ccusage plane and the
  overlay plane each carry their own age; a fresh overlay must never imply the local numbers are fresh.
- **Degrade silently to derived** is a real, testable requirement — not just a doc aspiration.
- **Single writer.** All `Store` writes stay on the collector loop task; overlay concurrency spawns
  only the *network fetches* (which touch no `Store`), harvested back into the loop for persistence.
- **`view` stays pure.** All freshness/liveness state is precomputed in the store read and lands on
  `App`; the view only styles it. Stale data must **look** stale (colour + word), per Grafana guidance.

## Behaviour

### A. Collector liveness (writer-down detection)

- `Store::heartbeat_age(component, now_ms) -> AppResult<Option<i64>>`: returns `now_ms - updated_at`
  for the component's heartbeat row, or `None` when the row is absent (never started).
- The TUI store read computes a `Liveness` for the `"collector"` component each tick and stores it on
  `App` (via `Dashboard`). Thresholds derive from `poll_local_secs` so they self-tune:
  - **Down** — heartbeat absent, or age `> DOWN_FACTOR × poll_local_secs` (default `DOWN_FACTOR = 3`).
  - **Live** — otherwise.
- `view::render` shows a prominent, single header state when the collector is **Down**:
  `"⚠ collector not running — data frozen (start `tok collector`)"`, or `"⚠ collector stalled — last
  beat Nm ago"` when a row exists but is old. It is styled critical (red/bold), width-degraded like the
  existing banner, and never overflows. When Live, nothing extra is shown.

### B. Local-plane freshness

- `AccountUsage` gains `collected_at_ms: Option<i64>` (the account's latest snapshot time). Populated
  in `account_usage` from `snapshot.collected_at.as_millisecond()`.
- `build_fleet_view` reduces the **newest** local `collected_at` across accounts into a local-plane
  age and always renders it when any account has a snapshot: `"usage <format_ago>"` (e.g.
  `"usage 12s ago"`), independent of the overlay.
- The existing overlay label is **scoped** so it can no longer imply local freshness: it renders as
  `"limits <format_ago>"` (overlay/authoritative plane) rather than a bare `"refreshed …"`.
- Past a staleness threshold (`STALE_FACTOR × poll_local_secs`, default `STALE_FACTOR = 2`) the local
  age segment is styled **warning** (yellow) instead of dim, so a stale number looks stale.
- `format_ago` sub-minute resolution is refined to seconds (`"12s ago"`) so a 10–20s cadence reads as
  live rather than collapsing to `"just now"` for the whole first minute.

### C. Degrade stale authoritative → derived

- A pure helper `demote_stale_authoritative(current, overlay_age, ttl) -> Vec<Limit>`: when the last
  overlay success is absent or older than `ttl`, demote every `Authoritative` row in `current` to
  `Estimate` (so the freshly derived session wins `merge_limits`, while the weekly/scoped gauges keep
  showing the last-known percent with a live countdown from the stored `resets_at` — never an `n/a`
  fallback for data we already have); otherwise return `current` unchanged. A demoted row whose reset
  then passes goes dormant via the spec 012 §A expiry machinery, so nothing alarms forever.
- `apply_limits` consults `store.last_overlay_success(account_id)` and applies the helper to the stored
  set **before** merging incoming limits. `ttl = TTL_FACTOR × poll_overlay_secs` (default
  `TTL_FACTOR = 2`). This is one guard at the shared merge point, so it covers both callers (local
  derived tick and overlay success) and all three failure modes (stale token, persistent 429, transport
  failure).

### D. Overlay off the critical path + concurrent

- On an overlay tick the collector spawns **one network fetch task per opted-in, warm-token,
  not-cooling-down account** into a `JoinSet` (each still bounded by the reqwest 10s timeout). The
  tasks return the raw body (or a typed error) — they touch **no `Store`**.
- Fetch outcomes are harvested on a dedicated `select!` arm and parsed + merged + persisted **on the
  loop task** (single writer), reusing `apply_overlay_body` / `apply_limits` and the 429 backoff /
  cooldown / round-robin bookkeeping.
- The `OVERLAY_TICK_BUDGET_SECS` timeout is retained only as an **outer safety cap** on the spawn +
  harvest, never as the mechanism that decides how many accounts refresh. Result: `local.tick()` and
  collect-drain keep being serviced while fetches are in flight, and **all** eligible accounts refresh
  each pass instead of ~`budget / timeout`.

### E. Bounded aggregate + retention + WAL hygiene

- `aggregate_burn_history(n)` bounds the rows it scans to the last `n` distinct ticks (a `collected_at`
  subquery / window), not a full-table scan, and the query is supported by an index leading with
  `collected_at`.
- The collector runs a **retention sweep** on a slow cadence (the single writer): `DELETE FROM
  snapshots WHERE collected_at < :cutoff` keeping a bounded window (`RETENTION_DAYS`, default a few
  days — comfortably longer than the sparkline needs), then `PRAGMA wal_checkpoint(TRUNCATE)` so the
  `-wal` reclaims space. This bounds both the scan cost and disk growth.
- `Store::open` sets `PRAGMA synchronous = NORMAL` (the recommended durability/throughput point for a
  WAL monitoring store).

### F. Tick & child hardening

- `set_missed_tick_behavior(MissedTickBehavior::Skip)` on both collector intervals and the TUI tick, so
  a monitor resumes cadence "from now" after a suspend/stall instead of replaying history.
- `kill_on_drop(true)` on the ccusage `tokio::process::Command`, so a timed-out child is reaped rather
  than leaked.
- Default `poll_local_secs` lowered `20 → 10` (still ≥ the 5s floor) so the always-on local plane feels
  live; the reset countdowns and freshness ages are recomputed every draw regardless.

## Non-goals (deferred)

- Wrapping each per-tick read in one deferred read transaction (intra-frame consistency) — self-heals
  next tick, LOW.
- `PRAGMA data_version` change-detection to skip redundant reads — once (E) bounds the scan, the tiny
  indexed reads are cheap (research: keep the 1s loop, don't micro-optimize it).
- Keeping the last-good aggregate on a transient read error (currently `unwrap_or_default()` → empty
  bar) — LOW, cosmetic single-tick flicker.

## Acceptance criteria

1. `Store::heartbeat_age` returns `None` when unwritten and a positive age after a heartbeat; the TUI
   renders a collector-down state when the age exceeds `3 × poll_local_secs` or the row is absent. (A)
2. The fleet line shows a **local-plane** age driven by the newest snapshot `collected_at`, present
   even with the overlay off; the overlay label is distinctly scoped ("limits …"). Stale local age is
   styled warning. (B)
3. `demote_stale_authoritative` demotes authoritative rows to `Estimate` past the TTL and is a no-op
   when fresh; after a simulated stale overlay, an account's stored session limit reverts to `Derived`
   and the weekly rows survive as estimates with their last-known values. (C)
4. The overlay refreshes **N** opted-in accounts per pass (not ~2) and the local plane is not stalled
   by an overlay cycle: a loop test shows local snapshots persisted during an overlay pass, and all
   opted-in accounts receive authoritative limits within one pass. Single-writer preserved. (D)
5. `aggregate_burn_history` scans only the last `n` ticks (bounded by an index); a retention sweep
   deletes snapshots older than the window and the WAL is checkpointed. (E)
6. Both collector intervals and the TUI tick set `MissedTickBehavior::Skip`; the ccusage command sets
   `kill_on_drop(true)`; default `poll_local_secs` is `10`. (F)
7. `./check.sh` is green (fmt + clippy pedantic `-D warnings` + tests).
