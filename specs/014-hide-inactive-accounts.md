# Spec 014 — Hide inactive (unsubscribed) accounts

Status: **Done**

## Motivation

A subscription can end while its account block stays in `tokenomics.toml` (live case: one Max
account — cancelled, its overlay 429s forever, spec 011's "overlay stalled — check account"
flag correctly fires). Deleting the block erases the account's identity, colour, and history for a
subscription that may come back. What's wanted: **keep the account configured, stop monitoring it,
and get it off the board** — with a way to peek at it.

## Behaviour

### A. Config: `active` flag

- `[[account]]` gains `active: bool`, `#[serde(default)]` **true** — every existing config is
  unchanged. `active = false` marks the account inactive (unsubscribed / paused).
- No new validation rules: an inactive account still needs a valid id/label/config_dir (it is
  still an account), and duplicate-id rules apply unchanged.

### B. Collector: inactive accounts are not monitored

- The collector's local pass and the overlay pass both skip inactive accounts entirely — no
  ccusage/sessions collect, no overlay fetch (a cancelled account 429s every pass; polling it is
  waste and noise). `upsert_accounts` still records all accounts (identity), and the store keeps
  whatever rows the account last had (history is never erased by deactivation).
- Alerts: with no new evidence there are no new crossings; an inactive account never fires alerts.

### C. TUI: hidden by default, toggle to peek

- Inactive accounts are **excluded by default** from: the rendered rows, the alert banner and its
  worst-offender scan, the `N account(s) at or above warn` count, and the fleet header reductions
  (shared-usage, worst-provenance, oldest-refresh — a dead account must not pin "refreshed 3d ago"
  or a stale provenance badge on the fleet line forever).
- Key **`i`** toggles *show inactive*. When shown, an inactive account renders its last-known rows
  **dimmed** with an `inactive` tag in its title/label zone, and stays excluded from the banner,
  warn count, and fleet reductions (peeking is display-only). Footer/help documents the key; the
  footer text participates in the existing degrade-by-width order.
- Selection and scrolling operate over the *visible* set; toggling clamps the selection so it
  always lands on a visible row. With every account inactive and the toggle off, the board renders
  the existing empty-state gracefully (no panic, no phantom selection).

### D. CLI surfaces

- `tok accounts` lists all accounts, marking inactive ones (`… (inactive)`).
- `tok once` skips inactive accounts (they are not monitored; `--json` output contains only active
  accounts).
- `tok doctor` still reports inactive accounts (it is diagnostics) but labels them inactive and
  skips their overlay reachability probe (never poll an account we've been told is dead).

## Non-goals

- Auto-detecting inactivity (spec 011's "overlay stalled — check account" flag stays the detector;
  this flag is the user's manual confirmation).
- Pruning an inactive account's stored history.
- A TUI affordance to *edit* the flag — config stays the single source of truth.

## Acceptance criteria

1. `active` defaults true; `active = false` parses; existing configs parse unchanged. (A)
2. Collector skips inactive accounts on both cadences (fake-adapter test: no collect call, no
   overlay fetch for the inactive account; active accounts unaffected). (B)
3. With an inactive account present: default board omits its row; banner/warn-count/fleet
   reductions ignore it (a crit-level stored limit on the inactive account raises no banner). (C)
4. Pressing `i` shows the account dimmed + tagged `inactive`, still excluded from banner and fleet
   reductions; pressing `i` again hides it; selection stays on a visible row across both toggles.
   Snapshot coverage for the shown state. (C)
5. `tok accounts` marks inactive; `tok once --json` omits inactive; `doctor` labels inactive and
   skips its overlay probe. (D)
6. `./check.sh` green.
