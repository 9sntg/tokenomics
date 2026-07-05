# Agents

## Active Agents
| Agent | Role | Scope | Permissions |
|-------|------|-------|-------------|
| Claude Code | Primary dev | Full codebase | Read/write/execute |

## Agent Rules
- Work on the `dev` branch by default; never push to `main`.
- Follow the rules in `rules/` (start at `rules/_index.md`, route via `rules/crossroads.md`).
- Development is **spec-driven TDD**: one spec per wave in `specs/`; cycle is
  spec → 🔴 red → 🟢 green → ♻ refactor-for-specs → ♻ refactor-for-rules.
- Run `./check.sh` green before marking any wave done.
- Add a `CHANGELOG.md [Unreleased]` entry for every user-facing change, in the same commit.
- Every `.rs` file carries a `//!` header (Project / Module / Deps / Tested / Key responsibilities /
  Design constraints).
- Never log or print an OAuth access/refresh token. Cost is a notional proxy, never a bill.
- Write handoffs to `docs/handoff/` when pausing mid-task.
