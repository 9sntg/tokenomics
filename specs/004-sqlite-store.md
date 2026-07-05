# Spec 004 — SQLite Store

**Status:** Done
**Wave:** 4
**Related:** `rusqlite` (`bundled`), `RESEARCH.md` §"good bones", `src/store.rs`

## Why
Persist snapshots/limits so history (sparklines) survives restarts and the collector (writer) and TUI
(reader) are decoupled. WAL lets a writer and readers coexist.

## Schema (WAL, `PRAGMA user_version` migrations)
- `accounts(id PK, label, provider, config_dir, color, limits_overlay)`
- `snapshots(id, account_id, collected_at, input, output, cache_read, cache_creation, total_tokens, cost_notional, win_start, win_end, win_remaining_secs, burn_tpm, burn_cph)`
- `limits(id, account_id, kind, scope, utilization_pct, resets_at, severity, source, collected_at)`
- `token_state(account_id PK, expires_at, last_refresh_at, status)` — `warm | stale | refresh_failed`
- `heartbeat(component PK, pid, updated_at)`

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `open(path)` | create tables if absent; `journal_mode = WAL`, `foreign_keys = on`, `busy_timeout`; idempotent migrations keyed by `user_version`. |
| FR-2 | `upsert_accounts(&[Account])` | reconcile the `accounts` table from config. |
| FR-3 | writers | `insert_snapshot`, `insert_limits(&[Limit], collected_at)`, `heartbeat`. (`insert_limits` takes an explicit `collected_at` since `Limit` carries no timestamp.) |
| FR-4 | `latest_snapshot(id)` / `latest_limits(id)` | last-good per account (`latest_limits` = all rows at the account's newest `collected_at`; provider joined from `accounts`). |
| FR-5 | `burn_history(id, n)` | last `n` `total_tokens` points, oldest → newest, for the sparkline. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | Tests use a `tempfile` DB; round-trip insert → latest; migration idempotent (open twice). |
| NFR-2 | Never panics on a busy DB (`busy_timeout` set); typed `AppError` on failure. |

## Deferred
- **`set_token_state` writer + `token_state` reads → Wave 7.** The `token_state` table is created in
  the v1 schema, but there is no token data to write until the overlay/token strategy lands.

## First consumer (landed early)
- **`tok collector` (single pass):** collect every account → `upsert_accounts` + `insert_snapshot` +
  `insert_limits` + `heartbeat`, then read back `latest_snapshot`/`latest_limits`/`burn_history` and
  print a per-account summary. This makes the whole store surface live now (no dead code under
  strict lints); **Wave 5 wraps it in the cadence loop + daemon guards.** Store path is
  **cwd-independent** (`src/paths.rs`): `$TOKENOMICS_DB` if set (non-empty), else the XDG data dir
  (`~/.local/share/tokenomics/tokenomics.db`) — no repo-/cwd-relative `./tokenomics.db` pickup.

## Acceptance Criteria (rollup)
FR-1..FR-5 (minus deferred `set_token_state`); `tempfile` round-trip + idempotent-open tests green;
`tok collector` proven to persist across processes (history accumulates); `check.sh` green.
