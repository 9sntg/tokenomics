# Spec 008 — Alerts

**Status:** Done
**Wave:** 8
**Related:** `src/alerts.rs`, `notify-rust`

## Why
Tell the user when an account crosses a threshold — **once**, not every tick.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `evaluate_alerts(prev_sev, curr_sev)` (pure) | fire on an upward crossing (Ok→Warn, Warn→Crit, Ok→Crit); no re-fire while unchanged; emit a recovery event on a downward crossing. |
| FR-2 | Banner is source of truth | the in-TUI banner is driven by model state, not the notifier. |
| FR-3 | Desktop notify | best-effort via `notify-rust`; failure is non-fatal and never blocks/crashes. |
| FR-4 | Cooldown | per-account, per-window cooldown so re-crossings don't spam. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | Evaluator pure & table-tested (all transitions). |
| NFR-2 | Notify errors swallowed with a debug note, never surfaced as a crash. |

## Notes
- The evaluator/tracker live in `alerts.rs`; the collector fires them from the shared `apply_limits`
  write path (per `(account, kind, scope)`), comparing the stored severity to the new one — so both
  the local (derived) and overlay (authoritative) planes alert consistently.
- The in-TUI banner (Wave 6) is the source of truth; the desktop notify is additive and best-effort
  (swallowed on WSL when no notification daemon is present).

## Acceptance Criteria (rollup)
FR-1..FR-4; transition table + cooldown tests (injected clock); notify errors non-fatal; `check.sh` green.
