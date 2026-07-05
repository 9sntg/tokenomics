# Spec 009 — E2E Verification & Doctor

**Status:** Done
**Wave:** 9
**Related:** `RESEARCH.md` §Verification, `tests/cli.rs`

## Why
Prove it works end-to-end against the real 3 accounts, and give a read-only diagnostics command.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `tok once [--json]` | collect one snapshot + limits per account; human table or normalized JSON; overlay-off ⇒ `source: "derived"`, weekly `"n/a"`. |
| FR-2 | `tok doctor` | read-only: per account report config_dir exists, `.credentials.json` present + mode `0600`, ccusage version, `CLAUDE_CONFIG_DIR` round-trip distinctness, overlay reachability (only if opted-in). **No secrets printed.** |
| FR-3 | Refresh hook (optional) | a Claude Code `SessionStart` hook / `tok-refresh` skill scaffold in `docs/` that warms an account token when active (documented, not required for v1). |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | `doctor` is strictly read-only; deterministic under fixtures for parseable parts. |
| NFR-2 | `once --json` output is stable & serde-typed. |

## QA runbook (manual, real accounts)
1. `tokenomics.toml` with 3 accounts (distinct `config_dir` per email, on ext4, creds `0600`).
2. `tok validate` → 0 errors; `tok accounts` → 3.
3. `tok doctor` → `CLAUDE_CONFIG_DIR` round-trip **distinct** (the Wave 2 gate); creds `0600`.
4. `tok once --json` → 3 normalized snapshots.
5. `tok` → 3 panels; % gauges; reset countdown; notional-cost label; sparkline persists across restart.
6. Overlay: enable on one warm account → badge derived→authoritative, weekly appears; kill network / force
   429 → silent fall back to derived, collector alive; expire a token → `"stale → open Claude"`.
7. Alerts: low threshold → banner once, clears on recovery.
8. `check.sh` green; run **code-simplifier** + **code-reviewer**, apply findings; CHANGELOG updated.

## Status of the runbook (this build)
- **Single-account, live-verified:** `tok validate` (0 errors), `tok accounts`, `tok once`/`--json`
  (normalized snapshot; `source:"derived"`, weekly n/a with overlay off), `tok doctor` (config_dir
  exists, creds present+`0600`+warm with **no token printed**, ccusage version, active block), and
  the dashboard rendered live against real store data in a PTY (`q` quits clean).
- **Needs the user (3-account + overlay + alerts):** items 1, 6, 7 of the runbook need the other two
  accounts each logged in under their own `CLAUDE_CONFIG_DIR` (only `~/.claude` exists here), and the
  overlay is a user-gated opt-in (flip `limits_overlay`, then `tok doctor` shows the round-trip
  distinctness and overlay reachability). These are documented, not run unattended.
- **FR-3** refresh-hook scaffold: `docs/token-refresh-hook.md` (SessionStart hook / periodic warm-up;
  documented, not required).

## Acceptance Criteria (rollup)
FR-1..FR-3; `doctor` read-only + secret-free; single-account runbook live-verified; 3-account/overlay
steps documented for the user; `check.sh` green.
