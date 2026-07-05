# Spec 015 — Config hot-reload + divergence surfacing

Status: **Done** (all acceptance criteria pass; `./check.sh` green)

## Motivation

Live incident (2026-07-15): one account was flipped back to `active` in `tokenomics.toml`
at 17:20, but the collector — started 09:43 — reads config **once at startup** and kept silently
skipping the account for 5+ hours (spec 014 makes the inactive-skip deliberately quiet: no polls,
no failure rows). The TUI, freshly launched against the *new* config, rendered the account with
`waiting for reset` / `n/a (waiting for overlay)` indefinitely. Nothing anywhere said "the running
collector disagrees with the config file". Worse, the per-account stall flag (spec 011) only
advances on *failed fetch attempts* — an account the collector never attempts can't trip it.

Three gaps, three fixes:

- **A.** The collector never notices config edits → hot-reload on file change.
- **B.** No surface records which config the collector loaded → heartbeat carries the loaded
  config mtime; doctor compares it to the file.
- **C.** `n/a (waiting for overlay)` is dishonest for an account whose overlay *used to* succeed
  and has gone silent → age-aware hint.

## Behaviour

### A. Collector hot-reloads config on change

- On each **local tick**, the collector polls a config source for a changed config. Production
  trigger: any change in the file's (mtime, size, content hash) versus the last successful load —
  the file is a few KB, so reading + hashing (`std` `DefaultHasher`, within-process comparison
  only) per tick is negligible and closes the mtime-preserving-edit hole (`cp -p`,
  `rsync --times`) where a stat-only watch and the doctor mtime check would go blind *together*.
  Re-parse + **re-validate** (`config::validate` — the same rules as startup; a parseable
  `poll_local_secs = 0` must not reach `tokio::time::interval`) only on trigger.
- On successful reload: the working `Config` is **swapped whole** — accounts (add / remove /
  `active` flips / `limits_overlay` flips), alert thresholds, and poll cadences all take effect
  without restart. **No once-captured locals survive the swap**: the loop reads `cfg.accounts` /
  `cfg.settings.*` (and everything derived from them — overlay TTL, thresholds) at use sites, so
  a reload cannot leave the local pass and overlay pass split-brained. Changed cadences recreate
  the affected `tokio::time::interval`s through one shared builder that always re-applies
  `MissedTickBehavior::Skip` and floors the period at 1s (a recreated interval must not revert to
  `Burst` or panic on zero). The collector calls `store.upsert_accounts(&new)` after every
  successful reload (identity recorded, history never erased — spec 014 semantics unchanged).
- Harvest guard: an in-flight overlay/collect result that lands after a reload is dropped when
  its account is no longer present-and-active in the current config (never stamp a fresh success
  onto a just-deactivated account).
- On parse/validation failure: keep the last-good config, log one warning per distinct bad mtime
  (no per-tick spam), never crash the daemon.
- Per-account transient state (inflight guard, generation map, overlay backoff / cooldown /
  round-robin, alert cooldowns) is keyed by account id and already tolerates the set changing
  shape; it is **retained** across reloads — orphaned entries are harmless, new ids start at
  defaults. A newly-(re)activated account joins the next local tick and the next overlay pass
  naturally (plus the existing warm-token recovery path).
- The config source is **injectable** (same seam idiom as `Runner` / `UsageEndpoint` /
  `RateLimitsSource`) so collector tests drive reloads in-memory — no real file, no env mutation,
  no fs in unit tests.

### A2. The TUI hot-reloads config too

- The TUI is the *other* long-running process (it lives on a monitor for days) and today freezes
  its account list and thresholds at launch (`cmd_tui` loads once; the in-loop
  `reload_requested` path re-reads store rows, never the config file). The same mtime-poll
  applied on the TUI's existing tick swaps its `Config` whole: accounts appear/disappear,
  `active` flips take effect, warn/crit thresholds recolor — without relaunching. Parse failure:
  keep last-good, silently (the TUI is not a log surface; doctor reports invalid configs).
- Same injectable-source seam as §A so the event loop stays testable without fs.

### B. Heartbeat records the loaded config; doctor flags divergence

- The `heartbeat` table gains nullable `config_path` (the **resolved absolute path** the
  collector loaded) and `config_mtime` (epoch-ms) columns (additive `user_version`-gated
  migration — existing rows survive). The collector writes them at startup and after **every
  successful hot-reload**, via a writer that is physically separate from the per-tick heartbeat
  upsert — the per-tick `SET` never names the config columns, so it cannot clobber them (store
  test: heartbeat N times after recording, values survive).
- `tok doctor` stats the **recorded path** (never its own re-resolution of `$TOKENOMICS_CONFIG` —
  doctor and collector may run in different environments and must not compare mtimes of two
  different files; a recorded path that differs from doctor's own resolution is itself worth a
  note). Store absent, row absent, or columns NULL ⇒ no new output — never guess.
- False-positive gate: warn only when the collector demonstrably had time to reload and didn't —
  file newer than recorded **and** the heartbeat has ticked for longer than a few local cadences
  since the file's mtime. Inside that window doctor stays silent (the reload is simply pending).
  The warning text says what a persistent mismatch means: the edited config fails
  parse/validation, or the collector predates hot-reload.

### B2. Heartbeat records the running binary; doctor flags rebuilds

- Today's incident had a second half: nothing detects a collector running a binary older than
  what's on disk (`CARGO_PKG_VERSION` never bumps here, so version comparison is useless). The
  collector records `std::env::current_exe()`'s path and mtime in the heartbeat row at startup
  (nullable columns, same migration as §B). Doctor stats the *recorded path*'s current mtime:
  newer than recorded ⇒ "collector binary rebuilt after start — restart the collector". Absent
  data (old row, unreadable exe) ⇒ print nothing new — never guess.

### C. Age-aware overlay hint

- `AccountData` gains the account's last overlay success (`overlay_ms`) — already read in
  `read_account_row`, currently dropped for per-row rendering.
- `build_account_view`'s weekly-hint chain gains one branch: when the overlay **has** succeeded
  before but that success is older than the existing `OVERLAY_STALL_MS` threshold (and the stall
  flag hasn't tripped — no attempts being recorded), the hint reads
  `n/a (overlay silent <format_ago_ms>)`. A genuinely-never-succeeded account keeps the honest
  `n/a (waiting for overlay)`.
- `AccountView.weekly_hint` becomes an owned `String` (computed text); existing literal sites
  convert. No snapshot re-blessing is required unless a fixture is deliberately extended.

## Non-goals

- Watching the config with inotify/notify (a per-tick `stat` is enough at a 10–20s cadence; no new
  dependency).
- Hot-reloading anything outside `Config` (store path, provider adapters, endpoint wiring stay
  process-lifetime).
- Auto-restarting the collector or mutating config from the TUI — config stays the single source
  of truth, edited by the user.
- Demoting a frozen weekly gauge client-side (demotion stays collector-write-side per spec 011;
  hot-reload restores the write path, which is the actual fix).

## Acceptance criteria

1. With an injected config source, flipping an account `active = false → true` mid-run makes the
   collector include it in the next local pass **and** the next overlay pass without restart
   (fake-adapter test: collect + overlay fetch observed for the reactivated account). The reverse
   flip stops both. (A)
2. A reload that changes `warn_pct`/`crit_pct` affects subsequent severity computation; a reload
   that changes cadences recreates the intervals **and** every cadence-derived value (overlay
   TTL) — no once-captured local survives the swap. A recreated interval keeps
   `MissedTickBehavior::Skip` and a floor ≥ 1s. (A)
3. A failing reload — unparseable **or** parseable-but-invalid (`poll_local_secs = 0`, duplicate
   id) — keeps the last-good config running and does not crash; the warning fires once per
   distinct bad content. (A)
4. `heartbeat.config_path`/`config_mtime` are written at startup and updated on successful
   reload; N per-tick heartbeats never clobber them. Migration preserves existing heartbeat
   rows. An in-flight result harvested after a deactivating reload writes nothing for that
   account. (A/B)
5. `tok doctor` warns on divergence only outside the reload-pending window (collector ticked for
   longer than a few local cadences past the file mtime without recording it); it stats the
   **recorded** path, and prints nothing new when values match, the store is absent, or the
   columns are NULL (CLI test via `TOKENOMICS_CONFIG`/`TOKENOMICS_DB`). (B)
6. An account with a past overlay success older than `OVERLAY_STALL_MS` and no `weekly` row shows
   `n/a (overlay silent … ago)`; a never-succeeded account still shows
   `n/a (waiting for overlay)` (pure unit tests on `build_account_view`). (C)
7. With an injected config source, a mid-session account/threshold change reaches the running
   TUI's board within one tick; a bad reload keeps the last-good view. (A2)
8. Heartbeat records exe path + mtime at collector start; doctor warns when the recorded path's
   current mtime is newer, and stays silent when equal, absent, or unreadable. (B2)
9. `./check.sh` green.
