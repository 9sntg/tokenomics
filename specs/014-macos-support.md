# 014 — macOS support: tell the truth about the overlay

> Tokenomics already works on macOS. The only platform gap feeds a feature Anthropic's terms
> don't permit — so the fix is honesty, not a port.
>
> Status: **Draft** (the maintainer promotes, not the agent).
> Requested by the maintainer, 2026-07-16: run tokenomics on macOS, same treatment as fleetops.

## Goal

`tok doctor` reports the macOS credential/overlay situation accurately, and the docs stop sending
macOS users to paths and platforms that don't exist.

## What was verified live (2026-07-16, this machine)

**The ToS-safe core needs no port.** With `ccusage` installed and `tok collector` running, the
board is fully populated on macOS:

```
TOKENOMICS  ·  1 account(s)  ·  cost is notional (usage proxy, not a bill)
255.28M · $215.22 (notional) · 94.62M/h · usage 7s ago · derived
╔ ▶ Personal [claude] ═══
║5h ███████████████████████████▎░░░░░░░ 64% ok · resets in 1h 46m
║wk n/a (enable overlay)
```

Nothing was Linux-bound: `paths.rs` resolves natively (config → `~/Library/Application
Support/tokenomics/`, store → the same), the SQLite store opens, `ccusage` is cross-platform, and
the `#[cfg(unix)]` blocks already cover macOS (`unix` ⊇ Darwin).

**The one platform gap** is `providers/claude/creds.rs`:

| | Linux | macOS |
|---|---|---|
| Claude Code's OAuth token | `<config_dir>/.credentials.json`, mode 0600 | **macOS Keychain** — service `Claude Code-credentials`, account = the macOS user |

The Keychain payload's JSON shape is **identical** to the file's (`claudeAiOauth.{accessToken,
expiresAt}` — verified by inspecting key names only, never values), so `parse_credentials` would
need no change. Only the source would move.

## Why the port is NOT done

The token exists in this codebase for exactly one caller: the opt-in `/api/oauth/usage` overlay.
Per the README's own notice:

> The overlay polls an **undocumented Anthropic endpoint** using your own consumer OAuth token.
> Per Anthropic's 2026 Consumer-Terms clarification, consumer OAuth tokens in third-party tools
> are **not permitted** — enabling the overlay is **at your own risk**, and account enforcement
> has occurred elsewhere in the ecosystem.

So a macOS Keychain reader would be code whose only purpose is to make a **not-permitted** action
easier, and — since the overlay is off by default — dead code until someone opts in. `rules/`
forbids dead code; CLAUDE.md's "Never" list forbids surprising the user about token handling.
**Decision: do not port it.** The local plane is complete without it.

A second, independent reason to be wary even if the terms changed: **the Keychain item is keyed
by macOS user, not by config dir.** Verified live — exactly one item exists, `acct` = the macOS
username. Tokenomics attributes accounts by `CLAUDE_CONFIG_DIR` (N accounts, N dirs), so a naive
macOS reader would hand *the same token* to every configured account, silently querying account
A's limits with account B's identity. <!-- ponytail: whether Claude Code writes a distinct
keychain item per CLAUDE_CONFIG_DIR is [NEEDS CLARIFICATION] — unverifiable here, the maintainer
has one account. Any future port MUST resolve this first; guessing mis-attributes. -->

## Behaviour

1. **`tok doctor` on macOS** reports, per Claude account:
   `credentials: n/a on macOS (Claude Code stores the token in the Keychain) · overlay unsupported`
   — replacing today's `credentials: cannot stat …/.credentials.json: entity not found`, which
   reads as a broken thing to be fixed and nudges the user toward a not-permitted feature.
2. **It is not an error.** Doctor's exit code and every other lane are unaffected; the local plane
   is the product, and it is complete.
3. **On Linux the behaviour is unchanged** — this is additive, not a platform swap. Tokenomics
   stays cross-platform (unlike fleetops, nothing here needed deleting).
4. **The overlay itself is untouched.** If an account sets `limits_overlay = true` on macOS it
   simply finds no token and degrades to derived — exactly what it already does on a 429 or a
   cold token. No new failure path.

## Seams & structure

- `creds::read_token` gains a macOS arm that returns a *typed, honest* `AppError::Credentials`
  naming the Keychain — rather than a confusing `cannot stat` from a file that was never going to
  exist. Pure parser untouched.
- `doctor::report_credentials` renders that state as information, not failure.
- No new dependency: the macOS arm is `#[cfg(target_os = "macos")]` and constructs a message; it
  never shells out to `security` (that would be the port we are declining).

## Deterministic tests (red first)

- 🔴 `creds::tests` — on macOS, `read_token` returns the Keychain-naming error, not a
  `cannot stat` one; the message names neither a path that will never exist nor any secret.
- 🔴 the existing parse/warmth/redaction tests stay green untouched (proving the pure core is
  platform-free and a future port would only move the source).
- 🟢 `./check.sh` green.

## Out of scope

- Reading the Keychain (see "Why the port is NOT done").
- Deriving a weekly window locally — it isn't in the logs; only the overlay has it. The safe way
  to see a weekly limit is Claude Code's own `/status`.

## Dependencies

**None added.**
