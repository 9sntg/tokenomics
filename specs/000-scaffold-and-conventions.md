# Spec 000 — Scaffold & Conventions

**Status:** Done
**Wave:** 0
**Related:** CLAUDE.md, AGENTS.md, `rules/rust/strict-lints.md`, `rules/file-headers.md`

## Why
Establish the strict, house-style Rust harness so every later wave has one gate (`check.sh`) and one
set of conventions. No product behavior yet beyond the CLI surface.

## Requirements

### Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| FR-1 | `tok --help` / `-h` | Prints usage listing validate/accounts/once/collector/doctor; exit 0. |
| FR-2 | `tok --version` / `-V` | Prints `tok <CARGO_PKG_VERSION>`; exit 0. |
| FR-3 | Unknown command | `tok <x>` prints "unknown command" to stderr; exit 2. |
| FR-4 | Unimplemented commands | validate/accounts/once/collector/doctor exit 2 with a "not implemented yet" notice until their wave lands. |

### Non-Functional
| ID | Requirement | Acceptance criteria |
|----|-------------|---------------------|
| NFR-1 | Strict lints | `[lints]`: `unsafe_code = forbid`, clippy all + pedantic `deny`; each allow carries a reason comment. |
| NFR-2 | Gate | `check.sh` = `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test`, green. |
| NFR-3 | Toolchain | edition 2021; `rust-toolchain.toml` channel stable; rustfmt `max_width = 100`. |
| NFR-4 | Headers | every `.rs` carries the `//!` header block (Project/Module/Deps/Tested/responsibilities/constraints). |
| NFR-5 | Rules vendored | `rules/` present (`_index.md` + `crossroads.md` + `rust/*`). |
| NFR-6 | Repo hygiene | `dev` default branch; `.gitignore` excludes `/target` + `*.db*`; CHANGELOG `[Unreleased]`. |

## Boundaries
- **Always**: `check.sh` green. **Never**: clap (house style = hand-rolled dispatch); `unsafe`.

## Acceptance Criteria (rollup)
Done when FR-1..FR-4 + NFR-1..NFR-6 hold, `tests/cli.rs` green, `check.sh` green. ✅
