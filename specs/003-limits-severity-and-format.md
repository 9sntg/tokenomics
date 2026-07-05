# Spec 003 — Limits, Severity & Format

**Status:** Done
**Wave:** 3
**Related:** `src/format.rs`, `src/domain.rs`

## Why
Pure presentation logic shared by the TUI and `tok once`: severity from thresholds and human
formatting. All pure → fully table-tested, no I/O. Wired into `tok once`'s human output so the
whole toolkit is exercised end-to-end.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `severity_for(pct, warn, crit)` | `pct >= crit` ⇒ Crit; `>= warn` ⇒ Warn; else Ok; NaN/negative ⇒ Ok. The single classifier (moved out of `domain::Severity`); the collector's `derive_session_limit` calls it. |
| FR-3 | `format_pct` | `"37%"` (0 decimals; clamp lower bound 0; NaN ⇒ `"0%"`). |
| FR-4 | `format_reset(resets_at, now)` | `"in 2h 41m"` countdown (adds `"in Nd Nh"` past a day); already past ⇒ `"resetting…"`; **unparseable ⇒ render the raw string verbatim**. |
| FR-5 | `format_tokens` | `1_234_567` ⇒ `"1.23M"`, `12_345` ⇒ `"12.3K"`, `<1000` verbatim (integer math, exact digits). |
| FR-6 | `format_cost` | `"$1.70 (notional)"` — always the notional label. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | All pure & table-tested; no I/O. |
| NFR-2 | `resets_at` rendered verbatim when not parseable; never fabricated. |

## Deferred (moved to the wave that first consumes them — avoids dead code under strict lints)
- **FR-2 `merge_limits(existing, incoming)`** → **Wave 7.** Provenance precedence
  (Authoritative > Derived > Estimate; equal ⇒ newer wins) only has a second source to merge once
  the opt-in overlay lands; before then every limit is `Derived`.
- **FR-7 `severity_color` (→ ratatui `Color`)** → **Wave 6.** Co-located with the TUI, where
  ratatui becomes a dependency and the color is first rendered.

## Acceptance Criteria (rollup)
FR-1, FR-3..FR-6 table-tested and wired into `tok once`; `check.sh` green.
