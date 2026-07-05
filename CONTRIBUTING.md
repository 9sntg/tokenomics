# Contributing to tokenomics

Thanks for your interest. This is a small, passively maintained project — contributions are welcome,
reviewed best-effort.

## Before you start

**Open an issue before a large PR.** A quick discussion saves you from building something that
doesn't fit the design. Small fixes (typos, obvious bugs) can go straight to a PR.

## The gate

`./check.sh` must be green before anything is considered done. It runs exactly what CI runs:

```bash
./check.sh    # cargo fmt --check + clippy (pedantic, -D warnings) + cargo test
```

No warnings, no formatting drift, all tests passing.

## How we work

Development is **spec-driven TDD**: one spec per wave in `specs/`, cycled
red → green → refactor-for-specs → refactor-for-rules. Coding rules live in `rules/` (start at
`rules/_index.md`). See [`CLAUDE.md`](CLAUDE.md) for the full workflow, conventions, and boundaries,
and add a `CHANGELOG.md` `[Unreleased]` entry for any user-facing change in the same PR.

## Be respectful

Be kind and constructive. Assume good faith. We're all here to make a useful tool.

## License of contributions

Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion in the
work, as defined in the Apache-2.0 license, shall be dual-licensed under MIT OR Apache-2.0 (the
project's license), without any additional terms or conditions.
