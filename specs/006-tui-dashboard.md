# Spec 006 — TUI Dashboard

**Status:** Done
**Wave:** 6
**Related:** `rules/rust/ratatui-architecture.md`, TUIservers `src/tui/*`

## Why
The board: one panel per account, reading the store. Three seams (`model`/`view`/`keys`); pure `view`
over `App`; `TestBackend` snapshots.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | Per-account panel | label + provider; 5h `Gauge` (% + color by severity); weekly `Gauge` or `"n/a (enable overlay)"`; provenance `Badge` (authoritative/derived/estimate); token-burn `Sparkline`; reset countdown; notional-cost label. |
| FR-2 | Alert banner slot | top banner when any account is Warn/Crit. |
| FR-3 | Keymap | `q`/`Esc` quit; `↑`/`↓`/`j`/`k` select account; `r` force refresh; `?` help. |
| FR-4 | `model.update` | folds `Msg` (Tick, StoreUpdated, Key, Resize); selection clamps to account count. |
| FR-5 | Responsive layout | pick a **density tier** from the body `Rect` (never overflow, never squeeze a bordered panel to an empty box): **FULL** bordered panels when each account gets ≥6 rows and width ≥72; **COMPACT** borderless spine-grouped 3-line blocks in the mid range; **MICRO** one aligned line per account when tiny. Overflowing accounts scroll a window that keeps the selection on-screen with `▴/▾ N more` chips; title/banner/footer degrade by width. |
| FR-6 | No I/O in view/update | `view`/`update` are pure; the loop re-reads the store on a ~1s tick (and on `r`), folding results as `Msg::Data`. **Data plane:** the TUI is a **reader** — the collector *process* writes the store (CLAUDE.md), rather than the TUI spawning collection in-process (which would corrupt the terminal with the collector's stderr). |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | `keys::map` + `model::update` + `severity→color` table-tested; `view` snapshot-tested with `TestBackend(120,40)` + `insta` over a 3-account fixture. |
| NFR-2 | Panic hook + restore via `ratatui::try_init`/`try_restore` (installs the hook; restores on panic and on early-return). |
| NFR-3 | `NO_COLOR` honored (resolved once at startup → `resolve_color`, table-tested). |

## Reconciliations
- **FR-5 responsive tiers (design revision):** the original "stack fixed panels" broke in small
  windows — ratatui squeezed the lower panels down to empty borders. Replaced with three density
  tiers sharing one invariant row grammar (marker · severity glyph · proportional bar · percent ·
  verbatim reset), selected purely from the body `Rect` via `choose_tier`. FULL preserves the roomy
  bordered look (now with visible eighth-block bars and a double-line border marking selection);
  COMPACT drops the borders for a left accent spine; MICRO collapses each account to one aligned
  line. Nothing depends on colour: severity is a glyph+word pair, bar fill is by character
  (`█`/`▏…▉` vs `░`), and selection is structural — so `NO_COLOR` renders a byte-identical grid.
  Snapshot-tested at 120×40 / 80×16 / 58×9 / 42×14, plus a 6-account scroll case.
- **FR-6 data plane:** implemented reader-only (see above). The separate `tok collector` daemon is
  the writer; run it alongside `tok` (or via the systemd unit).
- **`severity_color` (spec 003 FR-7):** landed in `tui/model.rs` (co-located with the TUI, where
  ratatui is a dependency and the colour is first rendered), not `format.rs`.
- **Weekly gauge:** shows `"n/a (enable overlay)"` until the overlay lands (Wave 7).

## Acceptance Criteria (rollup)
FR-1..FR-6; keymap/update/colour tests + insta board snapshot green; `tok` launches and renders
panels against real store data (verified live in a PTY; `q` quits clean); `check.sh` green.
