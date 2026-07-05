# Spec 001 — Config & Accounts

**Status:** Done
**Wave:** 1
**Related:** `RESEARCH.md` §CLAUDE_CONFIG_DIR, `src/config.rs`, `src/domain.rs`

## Why
Everything iterates the account list. Parse and validate `tokenomics.toml` into a typed `Config`,
**purely**, so the whole surface is table-testable. The config is the single source of the accounts,
each pinned to its own `CLAUDE_CONFIG_DIR` — the only multi-account attribution handle.

## Schema (`tokenomics.toml`)
```toml
[settings]
poll_local_secs   = 20      # ccusage cadence (floor 5)
poll_overlay_secs = 300     # overlay cadence (floor 60)
warn_pct = 75.0             # severity thresholds
crit_pct = 90.0
ccusage_cmd = ["npx", "ccusage"]  # optional launcher (Wave 2); omit ⇒ bare `ccusage` on PATH

[[account]]
id = "claude-personal"      # unique, non-empty
label = "Personal"          # display name, non-empty
provider = "claude"         # enum; unknown => error
config_dir = "~/.claude"    # must exist on disk; ~ expanded
color = "cyan"              # optional; a named ratatui color
limits_overlay = false      # opt-in, default false
```

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | Parse | `serde` + `toml` with `deny_unknown_fields`; `~`/`$HOME` expanded in `config_dir`. An unknown `provider` value is a **parse error** (typed enum) → exit 2, not a validate finding. |
| FR-2 | Resolution | **cwd-independent** (`src/paths.rs`): load `$TOKENOMICS_CONFIG` if set (non-empty), else `$XDG_CONFIG_HOME/tokenomics/tokenomics.toml` (`directories`). No repo-/cwd-relative pickup — a TUI must resolve the same file launched from any directory; the env var is the sole override (dev + tests). |
| FR-3 | `validate` (pure) | findings for: duplicate `id`; empty `id`/`label`; unparseable `color`; `crit_pct <= warn_pct`; poll intervals below floor; zero accounts. |
| FR-4 | `validate_environment` | thin fs check: each `config_dir` exists (mirrors groundcontrol's split so `validate` stays pure). |
| FR-5 | `tok validate` | prints findings; exit 0 if none are errors, 1 if any error, 2 if the config is unloadable/unparseable. |
| FR-6 | `tok accounts` | lists id, label, provider, config_dir, overlay flag; exit 0. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | `validate` is pure (`&Config` in, findings out; no I/O). |
| NFR-2 | Parse + each error variant covered by table tests; a golden valid config parses. |
| NFR-3 | Config holds no secrets; nothing sensitive is printed. |

## Boundaries
- **Always**: `deny_unknown_fields` (typos are errors). **Ask first**: adding config keys beyond this schema.

## Acceptance Criteria (rollup)
FR-1..FR-6 + NFR-1..NFR-3; parse/validate table tests + `tests/cli.rs` validate/accounts green; `check.sh` green.
