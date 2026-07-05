# Spec 007 — Authoritative Overlay (opt-in) & Token Strategy

**Status:** Done
**Wave:** 7
**Related:** `RESEARCH.md` §3.3 + §Token-freshness, `src/providers/claude/{overlay,creds,refresh}.rs`

## Why
The only source of real 5h + weekly utilization % and reset timestamps — but undocumented, 429-prone,
ToS-gray. **Strictly opt-in**, provenance-tagged, and it MUST degrade to derived without crashing.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `parse_oauth_usage` (pure) | map `five_hour`/`seven_day`/`seven_day_opus`/`seven_day_sonnet` + `limits[]` → `Vec<Limit> { source = Authoritative }`; `resets_at` verbatim. |
| FR-2 | `UsageEndpoint` trait + reqwest impl | `GET https://api.anthropic.com/api/oauth/usage`; `Authorization: Bearer`; `anthropic-beta: oauth-2025-04-20`; `User-Agent: claude-code/<detected ver>`; request timeout. |
| FR-3 | `creds::read_token(config_dir)` | read `.credentials.json` → `claudeAiOauth { accessToken, expiresAt }`; require file mode `0600`; **never log the token**; `is_warm = expiresAt` in the future. |
| FR-4 | Token strategy | warm ⇒ use directly (passive reuse). expired/absent ⇒ set `token_state = stale` and surface "open Claude to refresh" in the TUI. **Active headless refresh is deferred** — see reconciliation. |
| FR-5 | `next_backoff` (pure state machine) | on 429 (no `Retry-After`) grow the interval; on Ok reset; cap; skip overlay for stale-token accounts. |
| FR-6 | Degrade-to-derived | any error/429/opt-out ⇒ fall back to derived limits; collector keeps running; last-good cached. |
| FR-7 | Opt-in gating | overlay polled only for accounts with `limits_overlay = true`. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | `parse` + `next_backoff` pure & table-tested (200 body, 429, malformed). |
| NFR-2 | Endpoint/refresh behind traits; tested with canned 200/429/expired-token, no network. |
| NFR-3 | Tokens never appear in logs, errors, or the store; creds mode checked. |

## Boundaries
- **Ask first**: enabling the overlay by default; a scheduled headless refresh loop.
- **Never**: hammer the endpoint; block the collector on refresh; write a Claude config beyond an atomic token rotation.

## Reconciliations
- **FR-4 active refresh → deferred (Ask-first).** Implemented **passive token reuse only** (the
  ToS-cleanest path): warm ⇒ use; expired ⇒ `token_state = stale` + a TUI hint. A best-effort
  headless refresh (POST + atomic rotation) writes to a Claude config dir and is a "Ask first"
  boundary, so it is a documented future opt-in — **not** built now. Nothing here ever mutates the
  credentials file.
- **FR-1 `limits[]` (revised — the endpoint moved on).** The live endpoint now carries a canonical
  `limits[]` array (`kind` = `session`/`weekly_all`/`weekly_scoped`, with a per-model `scope`), and
  the old flat `seven_day_opus`/`seven_day_sonnet` fields are `null`. `parse_oauth_usage` therefore
  **prefers `limits[]`** (mapping scoped weeklies by model `display_name`, e.g. `Fable`) and falls
  back to the flat `five_hour`/`seven_day` windows only when the array is absent. Unknown `kind`s are
  skipped, so new limit types don't break the parse. The TUI renders weekly-all and the most-utilized
  scoped weekly as their own gauges (§006), and a row's severity is the worst of all its limits.
  **`resets_at` is nullable** (seen live 2026-07-08: an idle account with no active 5h session sends
  `"resets_at": null` on the session entry) — a null maps to `""` (gauge renders with no countdown
  tail) instead of failing the whole body, which used to discard the weekly rows exactly when an
  account was pinned at its weekly limit.
- **Per-account refresh time.** The store records the last successful authoritative fetch per account
  (`overlay_state`, schema v2), written only on the overlay success path — never by a derived tick,
  which re-stamps `limits.collected_at` every cadence. The TUI reads it and shows `refreshed Nm ago`,
  making it visible that a stale-token account (not the one currently logged in) is showing frozen
  numbers. Each gauge also renders its own reset (5h + weekly), so both reset times show per account.
- **Overlay is orthogonal to `ProviderAdapter`.** `fetch`/parse/merge live in `overlay.rs` + the
  collector's overlay tick (rather than an `adapter.fetch_limits`), keeping the tested collector
  core and adapter generics untouched.
- **`merge_limits` (spec 003 FR-2)** landed here (its second source, the overlay, now exists); the
  store uses merge-at-write so a derived tick never clobbers an authoritative row.
- **Live proof.** Degrade-to-derived is proven by canned 429/failure tests + the opt-in integration
  test (real 0600 creds + canned 200 → authoritative limits). A **live** call to the real endpoint
  fires the user's OAuth token at an undocumented ToS-gray API, so it is a user-gated step (flip
  `limits_overlay = true`), not run unattended.

## Acceptance Criteria (rollup)
FR-1..FR-3, FR-5..FR-7 + passive-reuse FR-4; pure tests (parse/backoff/merge/creds) + canned-endpoint
+ opt-in integration test; token never logged/stored; creds mode enforced; `check.sh` green.
