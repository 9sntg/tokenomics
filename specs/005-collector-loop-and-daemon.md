# Spec 005 — Collector Loop & Daemon

**Status:** Done
**Wave:** 5
**Related:** ghmonitor `update.go` (inflight + generation guards), `src/collector.rs`

## Why
Drive all accounts on two cadences, writing to the store, with the correctness guards that stop ticks
stacking and drop stale async results. Runnable headless as `tok collector` (the 24/7 writer).

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | Local tick | every `poll_local_secs`, each account: `adapter.collect` → `insert_snapshot` + derived `insert_limits` (fast, ToS-safe). First tick fires immediately. |
| FR-3 | Inflight guard | never start a second collect for an account while one is inflight (skip, don't queue). |
| FR-4 | Generation guard (pure `should_apply`) | each spawn stamped with a monotonic gen; a result is applied only when strictly newer than the last applied for that account. |
| FR-5 | Isolation | a failing account retains last-good and never crashes the loop or blocks other accounts (per-account error is logged; loop continues). |
| FR-6 | `tok collector` | runs the loop headless, writes `heartbeat` each tick, honors SIGINT/SIGTERM for clean shutdown (prints `collector: stopped`, exit 0). |
| FR-7 | Bounded concurrency | at most `MAX_INFLIGHT` (8) collects in flight across accounts; excess accounts wait for the next tick. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | Guard predicates pure & table-tested; loop thin-async, tested with a fake adapter (3 accounts, canned snapshots → all three persisted). |
| NFR-2 | Ships a `docs/` example `systemd --user` unit (`docs/running-the-collector.md`; optional to install; not auto-installed). |

## Deferred
- **FR-2 Overlay tick → Wave 7.** The overlay cadence needs `adapter.fetch_limits` + the warm-token
  gate, both of which are the Wave 7 overlay. Until then the local tick's derived limits stand.

## Landed alongside
- **`tok collector --once`:** the single-pass collect→persist→read-back (from Wave 4), kept as a
  cron-friendly mode next to the default daemon loop.

## Acceptance Criteria (rollup)
FR-1, FR-3..FR-7 (FR-2 → W7); `should_apply` table + fake-adapter loop test; daemon proven against a
real account (immediate tick persists; SIGTERM stops clean); `check.sh` green.
