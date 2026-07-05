# Spec 010 — Aggregate burn bar + per-account tokens/hour

Status: **Active**

## Motivation

The FULL-tier panels each carried a per-account token-burn **sparkline** (`store.burn_history`, the
active block's cumulative `total_tokens` over time). Two problems, one design and one environmental:

1. **It plots a cumulative sawtooth, not activity.** `total_tokens` ramps within a 5h block then
   resets, so the bar reads as a solid left→right gradient wedge — not a burn-rate trace.
2. **All accounts currently render identically.** In this deployment every account's
   `<config_dir>/projects` is a symlink to the shared `~/.claude/projects`, so ccusage reports the
   same totals for every `CLAUDE_CONFIG_DIR`. Per-account attribution is impossible until each
   account has its own real `projects/` — an environment change, out of scope for this repo.

Given (2) is accepted for now (aggregate-only decision), this wave replaces the four identical
per-panel strips with **one fleet-wide aggregate burn-rate bar** in the header, keeps per-account
numbers (they stay shared, and `doctor` now says why), and **adds a per-account tokens/hour** figure.

## Behaviour

### A. Aggregate burn-rate bar (header)

- The store exposes `aggregate_burn_history(n)`: for the last `n` collection ticks, `Σ burn_tpm`
  across all accounts at each `collected_at`, oldest → newest. Accounts are collected on one cadence
  so they share exact `collected_at` values; aggregation is `GROUP BY collected_at, SUM(burn_tpm)`.
- `App` carries `aggregate_burn: Vec<u64>`, refreshed alongside the rows each tick.
- `view::render` draws a one-row header bar (label `burn · all accts` + a `Sparkline`) below the
  title/banner when there is data, the help overlay is closed, and the terminal has height to spare.
- The bar is a **shape** signal only — no absolute number is printed (the summed `burn_tpm` is
  ccusage's cache-inclusive rate, a notional proxy; a bare figure would mislead and, while accounts
  share data, is inflated ×N).

### B. One fleet-wide usage line (header), not a per-account meta line

The per-account meta line — token count · notional cost · tokens/hour · provenance · refresh — read
**identically on every panel** (same shared logs as (2)), so repeating it four times added no
information. It is collapsed to **one fleet line** in the header:

- `build_fleet_view(usages, now, use_color) -> Option<FleetView>` reduces per-account `AccountUsage`
  facts (`account_usage`, extracted in the same store read as each row): token count / cost / burn
  are the **representative** shared value (`max`, never a sum — summing identical shared logs would
  inflate ×N); provenance is the **worst** (most degraded) across accounts; refresh is the **oldest**
  (most stale). Returns `None` when no account has data yet (the line is simply omitted).
- `view::render` draws it as a one-row line below the title/banner (above the burn bar) when there is
  data, help is closed, and the terminal has a spare row. It degrades by width — dropping refresh →
  provenance → burn, and shrinking `$X.XX (notional)`→`$Xn` and `derived`→`drv` — and never overflows.
- `AccountView` loses its `tokens` / `cost_notional` / `cost_dollars` / `burn_rate` / `provenance` /
  `refreshed` fields (now on `FleetView`); a per-account row carries only what **differs** between
  accounts: its gauges, headline, status, severity. FULL panels lose the meta row (`PANEL_HEIGHT`
  5→4, scoped 6→5); the COMPACT/MICRO tiers drop their per-row cost chip.
- The tokens/hour figure keeps the same cache-inclusive basis and `format_tokens` + `/h` formatting
  (e.g. `"271.20M/h"`); the old per-panel sparkline stays removed.

### C. Doctor: name the root cause

- `doctor` adds a cross-account check that canonicalizes each `<config_dir>/projects` and warns when
  two or more accounts resolve to the **same** realpath: per-account usage attribution is disabled
  because they share one `projects/` directory. Precise, deterministic — supersedes guessing from
  identical token totals (the existing `report_distinctness` heuristic stays as a fallback signal).

## Acceptance criteria

- `store.aggregate_burn_history(n)` returns the summed-per-tick series, oldest→newest, ≤ `n` points;
  `NULL` burn rates count as 0; empty store → empty vec. (unit-tested)
- `account_usage` extracts a snapshot's token/cost/burn + session provenance + overlay time;
  `build_fleet_view` reduces a slice of them to the representative usage, worst provenance, and oldest
  refresh, and returns `None` when no account has data. (unit-tested)
- The header renders one fleet usage line (not a per-panel meta line) and the aggregate bar when data
  is present; panels carry only per-account gauges/headline; the fleet line degrades by width and
  never overflows at any tier. (view tests + insta snapshots updated)
- `doctor` prints a shared-`projects/` warning when accounts share a realpath. (covered by the pure
  grouping helper)
- `./check.sh` green.
