# tokenomics — TUI Stack Decision

Decision date: 2026-07-04. Method: 4-agent judge-panel (balanced deep-dive per stack →
neutral weighted scoring), grounded in the user's real repos (`ghmonitor` Go/Bubble Tea,
`groundcontrol` Rust). Full research: `RESEARCH.md`.

## Verdict

**🥇 Go + Bubble Tea (151/165) — by forking `ghmonitor`.** Rust + ratatui (147) is a genuine
runner-up within ~3%, not a distant one. TypeScript + Ink (119) is eliminated.

| Dimension | Weight | Rust | Go | TS |
|---|:--:|:--:|:--:|:--:|
| Core-pattern fit (ccusage shell-out + injectable-runner seam) | 5 | 5 | 5 | 4 |
| TUI widgets (gauges / sparklines / charts) | 4 | 5 | 4 | 3 |
| HTTPS/OAuth overlay (authed GET + refresh, WSL Cloudflare WAF) | 3 | 4 | 4 | 5 |
| SQLite store (WAL, writer/reader split) | 3 | 5 | 4 | 3 |
| Concurrency (poll 3 accounts × M providers) | 1 | 4 | 5 | 4 |
| Distribution (single static binary + systemd daemon) | 4 | 5 | 5 | 3 |
| Velocity / maintainability (for this user) | 5 | 3 | 5 | 3 |
| Extensibility (Codex/Gemini behind one interface) | 4 | 4 | 5 | 5 |
| Longevity (multi-year set-and-forget) | 4 | 5 | 4 | 3 |
| **Weighted total** | | **147** | **151** | **119** |

Weights reward what a personal, long-lived, evening-built tool actually needs:
velocity/maintainability and core-pattern-fit (5); widgets, distribution, extensibility,
longevity (4); overlay + sqlite (3, table-stakes); concurrency (1, trivial at 3 accounts).
Ordering is robust to reasonable weight perturbation.

## Why Go wins — a checkable fact, not taste

The Go lane read `ghmonitor`'s source. `internal/gh/client.go` **is already** the exact seam
tokenomics needs: a typed data layer with an **injectable exec-runner** over a CLI, bounded
concurrency (`WithConcurrency` + sem-channel), and canned-fixture tests (runner is a field, no
network). Mapping to tokenomics:

- `gh` CLI → `ccusage blocks --json` (add `CLAUDE_CONFIG_DIR` to the runner's `cmd.Env` per account)
- `internal/gh/ratelimit.go` → Claude/Codex limits model
- `backend.go` → the `ProviderAdapter` interface for Codex/Gemini
- `internal/ui/*`, `internal/config`, `internal/version` → poll+render+config skeleton

⇒ Phase-1 collector core starts at **~90%**; a working 3-account TUI in **1–2 evenings**, in an
idiom the user already fluently maintains (`tea.Cmd`/`Msg`, `teatest`).

## The honest case for Rust (the runner-up)

On *finished-artifact* axes Rust wins, and this is why `groundcontrol` "feels best":

- **All five dashboard widgets built into ratatui**: Gauge / LineGauge / **Sparkline** / Chart /
  BarChart. Go's `bubbles` has progress + table + timer but **no sparkline/gauge** — needs
  single-vendor `ntcharts` or ~30 hand-rolled LOC.
- **`rusqlite` `bundled`** statically links SQLite with no cgo — the single cleanest static binary
  (Go's pure-Go `modernc.org/sqlite` keeps `CGO_ENABLED=0` but is ~2× slower — immaterial here).
- **Compiler-enforced correctness**: Rust enums make the `authoritative | derived | estimate`
  provenance model and the provider set exhaustive — adding Codex/Gemini is a change the compiler
  proves complete.
- **Longevity**: `reqwest`+`rustls`+`rusqlite` musl artifact has the fewest moving parts to rot;
  Go is pinned to the Charm v1 stack facing an in-flight v1→v2 migration.
- ghmonitor's head start is **front-loaded to Phase 1** — the overlay, SQLite, sparklines,
  notifications, and daemon are net-new in *every* stack, so Phase-3 effort converges (~1.5–2.5
  weeks of evenings) regardless of language.

## Why not TypeScript / Ink

Its one structural advantage ("stay in Node, import ccusage") is **dead** — ccusage v20 is a
native binary every language shells out to identically. Result: zero `ghmonitor` reuse, weakest
widget story (`@inkjs/ui` = ProgressBar/Badge only, no native chart), and a recurring
Node-runtime/native-addon ABI + distribution tax (`better-sqlite3` breaks `bun build --compile`)
on a multi-year daemon. The user's two real TUIs are already Rust and Go.

## Decision guide

- **Pick Go** to ship fastest — fork ghmonitor's verified runner-seam core, 3-account TUI with
  gauges + reset countdowns in 1–2 evenings, low-friction maintenance in a stack you already run.
- **Pick Rust** to optimize the multi-year artifact over time-to-first-pixel — built-in widgets,
  cleanest zero-dep static binary, compiler-enforced provenance/adapters, lowest long-term rot
  (the qualities that make `groundcontrol` feel best). Costs ~1 extra evening at Phase 1.
- **Effort shape**: Go fastest to a working daily tool and to Phase-3 parity; Rust slowest to
  first pixel but gets static-musl packaging free and is cheapest to keep alive for years; both
  converge by Phase 3. TS starts fast but pays recurring distribution/ABI tax and never gets the
  template head start.

## Chosen stack (pending final Go-vs-Rust confirmation)

- **Go**: `charmbracelet/bubbletea` + `bubbles` + `lipgloss`; `modernc.org/sqlite` (pure-Go, WAL,
  static binary); stdlib `net/http` for the overlay; `ntcharts` (or hand-rolled) sparklines;
  fork `ghmonitor`'s `client.go`/`config`/`ui` skeleton.
- **Rust** (if chosen): `ratatui` + `crossterm`; `rusqlite` (`bundled`); `reqwest`(rustls) or
  `ureq` for the overlay; `serde`/`serde_json` for ccusage JSON; `tokio` or threads for polling.
