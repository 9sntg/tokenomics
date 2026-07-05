# Spec 012 — Expired limits: "waiting for reset" + idle re-evaluation

Status: **Done** (all acceptance criteria pass; `./check.sh` green)

## Motivation

Two related staleness bugs survived spec 011 (observed on a live board: an account showing
`90% crit` / `100% crit` with `resets resetting…` **for days**, while the account's real usage —
per Claude's own `/usage` — was ~0%):

- **A. A past `resets_at` renders as a frozen live gauge.** When a limit's reset time passes, the
  window has reset and the stored `utilization_pct` is history, not state. The board nevertheless
  keeps drawing the old percent, the old severity colour, and counts the row toward the alert
  banner ("worst: Echo 100% crit") — indefinitely, until fresh data happens to arrive.
- **B. The spec 011 §C demotion never fires for idle accounts.** `demote_stale_authoritative` runs
  only inside `apply_limits`, which the collector calls only when *new evidence* lands (a snapshot
  with an active block, or an overlay success). An account that is idle (no active ccusage block ⇒
  `Ok(None)` ⇒ `apply_limits` never called) with a stale token (overlay skipped, per the "never
  poll a stale-token account" boundary) never re-evaluates its stored limits — so the frozen
  authoritative crit rows persist for days. The demotion invariant exists but its trigger is
  activity-gated: exactly the accounts that need it (idle, logged-out) never get it.

## Behaviour

### A. Expired limits render "waiting for reset" and stop alarming

- `format::reset_expired(resets_at, now) -> bool` (pure): `resets_at` parses **and** is at or past
  `now`. Unparseable ⇒ `false` (an unknown time is never treated as expired — verbatim rule).
- The past-reset sentinel `RESET_DONE` becomes **`"waiting for reset"`** (was `"just reset"`): the
  moment the countdown crosses zero, every tier reads `waiting for reset` until fresh evidence
  (a new collect, or an overlay success after the user logs in / opens Claude) replaces the row —
  at which point the new countdown shows automatically (the merge already does this).
- `gauge_from_limit` (model, pure): an expired limit renders a **dormant** gauge — ratio `0.0`,
  pct `"—"`, severity `Ok`, colour `DarkGray`, reset `RESET_DONE`, and a new `expired: bool` flag
  on `GaugeView`. The stale percent is never shown as if current.
- Label composition: an expired gauge's label is `"[scope ]waiting for reset"` in every tier (no
  `"— ok"` filler, no `"resets"` verb). The banner's worst-offender suffix uses the same verb drop.
- Row severity (`build_account_view`) is the max over **non-expired** limits only, so an
  all-expired account is `Ok` — it leaves the alert banner and the `N account(s) at or above warn`
  count the moment its resets pass.
- The MICRO reset column cap and the COMPACT tight right-zone widen to fit the sentinel whole
  (the never-mutilate-a-reset rule).

### B. Idle / failing collects still re-evaluate limits

- `apply_outcome` on `Ok(None)` (idle) and `Err` (failed collect) now calls `apply_limits` with an
  **empty** incoming set. The empty merge still runs `demote_stale_authoritative`, so a frozen
  authoritative set degrades once the overlay success ages past the TTL — for **every** account,
  active or idle. One guard at the shared merge point (no per-caller special cases).
- `apply_limits` early-outs (no write, no alert scan) when the merged set equals the stored set,
  so idle re-evaluation does not churn SQLite writes every local tick.

## Non-goals

- Pruning expired rows from the store. The store keeps the last-known truth (provenance intact);
  expiry is a *display* fact recomputed per draw, and the collector's demotion handles the
  authoritative-staleness half. Deleting rows would erase "what we last knew" for no gain.
- Actively refreshing a stale token. Logging in / opening Claude rotates the credentials file; the
  collector re-checks not-warm opted-in accounts on the **local** tick (≤ `poll_local_secs`) and
  fires the overlay fetch the moment the file rotates warm — so a re-login is picked up in seconds,
  not after the next ≤ `poll_overlay_secs` overlay pass. Headless OAuth refresh itself stays out of
  scope (RESEARCH §8: Cloudflare WAF, rotating refresh tokens, refresh-race with a live CLI).

## Acceptance criteria

1. `reset_expired` is true for a past/now timestamp, false for future or unparseable. (A)
2. `format_reset` past ⇒ `"waiting for reset"`; an expired limit's gauge is dormant (ratio 0,
   pct `"—"`, severity `Ok`, `expired`) and its label is `"[scope ]waiting for reset"`. (A)
3. An account whose limits are all expired has row severity `Ok` (drops off the banner); a
   non-expired crit still wins. (A)
4. MICRO tier renders the sentinel whole (never truncated). (A)
5. With a stale overlay success and an **idle** adapter (`Ok(None)`), the collector demotes stored
   authoritative limits within one local tick — `latest_limits` keeps the rows, re-ranked
   `Estimate` (last-known values retained, authority dropped). (B)
6. `apply_limits` early-outs before `set_limits` when the merged set equals the stored set (no
   write churn from the every-tick idle re-evaluation). (B)
7. `./check.sh` green.
