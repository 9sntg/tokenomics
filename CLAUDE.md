# Tokenomics

Tokenomics is a single-binary Rust TUI that monitors LLM **subscription** accounts — Claude Max,
Codex, later Gemini / Grok. Per account it shows token **usage** (notional cost as a labeled proxy,
never a bill), **limit % utilization** (5h + weekly), and **time-left until reset**.

**Cross-platform (Linux + macOS).** Nothing here is Linux-bound: paths resolve per-platform, the
store is SQLite, and `ccusage` is cross-platform. One difference, by design (spec 014): on macOS
Claude Code keeps the OAuth token in the **Keychain**, not `<config_dir>/.credentials.json` — and
we deliberately do NOT read it, because the token's only consumer is the opt-in overlay that the
README documents as NOT PERMITTED under Anthropic's 2026 Consumer-Terms clarification. So on macOS
the weekly window reads `wk n/a` and the local plane is the whole product. Prereq: `ccusage` on
PATH (`npm i -g ccusage`) — without it every collect fails.

Background: `RESEARCH.md` (data sources, the `/api/oauth/usage` overlay, `CLAUDE_CONFIG_DIR`
attribution, ToS) and `STACK-DECISION.md` (why Rust + ratatui).

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Language | Rust (2021, strict — `forbid(unsafe_code)`, clippy pedantic `-D warnings`) |
| TUI | ratatui + crossterm |
| Async | tokio (never block the UI task; async results arrive as messages over channels) |
| Local data | ccusage CLI (`blocks --json`) per account via `CLAUDE_CONFIG_DIR`; direct-JSONL fallback |
| Store | SQLite (rusqlite `bundled`, WAL) — collector writes, TUI reads |
| Limits overlay | reqwest (rustls) → `GET /api/oauth/usage` (opt-in, provenance-tagged, degrades to derived) |
| Config | TOML (`serde` + `toml`) in the platform config dir (Linux `~/.config/tokenomics/`, macOS `~/Library/Application Support/tokenomics/`) |
| Time | jiff (reset countdowns) |

## Commands

```bash
./check.sh                 # THE GATE: fmt --check + clippy -D warnings + test (must be green)
cargo run -- validate      # validate tokenomics.toml
cargo run -- accounts      # list configured accounts
cargo run -- once --json   # one snapshot per account, as JSON
cargo run -- collector     # run the background collector (writes the store)
cargo run                  # launch the TUI
cargo run -- doctor        # read-only diagnostics
cargo build --release      # -> target/release/tok
```

Binary is `tok`. Config + store live in the platform dirs (`directories::ProjectDirs`) — Linux
`~/.config/tokenomics/tokenomics.toml` + `~/.local/share/tokenomics/tokenomics.db`; macOS both under
`~/Library/Application Support/tokenomics/`. `tok init`/`doctor` print the resolved paths. Path
resolution is **cwd-independent** (`src/paths.rs`) —
for dev, point at repo-local files explicitly:
`TOKENOMICS_CONFIG=./tokenomics.toml TOKENOMICS_DB=./tokenomics.db cargo run -- …`.

## Rules

**Read before writing any code.** All coding rules live in `rules/`. Start at `rules/_index.md`;
route via `rules/crossroads.md`. Every `.rs` file carries a `//!` module header per
`rules/file-headers.md`. Rust specifics: `rules/rust/{strict-lints,ratatui-architecture,
subprocess-safety,async-tokio,error-handling,anti-patterns}.md`.

## Specs

**Development is spec-driven TDD.** One spec per wave in `specs/` (index: `specs/README.md`).
Cycle per wave: **spec → 🔴 red → 🟢 green → ♻ refactor-for-specs → ♻ refactor-for-rules**. Mark
ambiguities `[NEEDS CLARIFICATION]`; never guess. Update the spec alongside the code when they diverge.

## Versioning

- Maintain `CHANGELOG.md` `[Unreleased]` — add an entry for every user-facing change, in the same commit.
- Never bump the version or cut a release — only the user does.

## Git

- **Default branch: `main`** — the public release moved to it; there is no `dev` branch on the
  remote (checked 2026-07-16). This line previously said "Default branch: `dev`. Never push
  directly to `main`", which no longer matched reality. Handoffs go in `docs/handoff/`.

## Architecture

**Two data planes, never conflated.** (1) Local ccusage / JSONL token usage = the ToS-safe core.
(2) The `/api/oauth/usage` overlay = **opt-in**, provenance-tagged, and degrades silently to derived
estimates on any 429/failure. Rendering is a pure function of state; the event loop is the only place
that does I/O — collection runs as tokio tasks that send results back as messages; `view` only reads
`App`. **Account attribution is the `CLAUDE_CONFIG_DIR`, never the logs** (logs carry no identity).

## Conventions

- Design seams so core logic is pure and testable: config, ccusage parse/reduce, severity/format,
  alerts, keymap — all unit-tested without touching the OS or network (see `src/providers/claude/ccusage.rs`).
- Shell out via explicit **argv** (never `sh -c`); every external call and HTTP request has a timeout.
- Cost is a **NOTIONAL proxy, never a bill**. Limits are **% + reset, never "X of Y"**. `resets_at`
  is rendered verbatim. Alerts key off `utilization_pct`, never cost.

## Boundaries

- **Always**: run `./check.sh` green before calling a wave done. Follow `rules/`. Update the spec + CHANGELOG.
- **Ask first**: new external dependency; enabling the overlay by default; anything that writes to a
  Claude config dir beyond an atomic token rotation.
- **Never**: `unsafe`. `unwrap`/`expect`/`panic!` in runtime paths. Log or print an access/refresh
  token. Poll the overlay for a stale-token or opted-out account. Present notional cost as a real bill.

## Key Files

| File | Purpose |
|------|---------|
| platform config dir `/tokenomics.toml` | Accounts + thresholds (source of truth); `tok init` prints it |
| `src/providers/claude/ccusage.rs` | ccusage JSON → `UsageSnapshot` (pure core) |
| `src/providers/claude/overlay.rs` | `/api/oauth/usage` parse + backoff (opt-in) |
| `src/domain.rs` | `Account` / `UsageSnapshot` / `Limit` / `Provenance` contracts |
| `src/store.rs` | SQLite (WAL) — collector writes, TUI reads |
| `rules/_index.md` | Coding rules index · `rules/crossroads.md` task routing |
| `specs/README.md` | Spec index (one per wave) |
| `CHANGELOG.md` | `[Unreleased]` history |
