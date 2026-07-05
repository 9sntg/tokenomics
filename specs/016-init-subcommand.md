# Spec 016 — `tok init` starter config

Status: **Done** (all acceptance criteria pass; `./check.sh` green)

## Motivation

Every other command (`validate`, `accounts`, `once`, `collector`, `doctor`, the TUI) fails on a
fresh machine with `cannot read config at …/tokenomics.toml: No such file or directory` and no next
step — the user has to know the path, the schema, and that the overlay is a deliberate opt-in before
anything runs. The README already tells people to start with `tok init`; the command has to exist and
hand them a config that is correct by construction.

## Behaviour

### A. `tok init` writes a starter config

- Writes a commented starter `tokenomics.toml` to the **resolved config path**
  ([`paths::config_path()`] — `$TOKENOMICS_CONFIG` if set, else the XDG config path), creating parent
  directories as needed, and exits `0`.
- The starter content is embedded from `tokenomics.example.toml` at the repo root via
  `include_str!` — the file and the subcommand are the **same bytes**, so they can never drift. The
  README's fenced example block mirrors that file.
- The generated config **parses and validates cleanly**: one active Claude account
  (`config_dir = "~/.claude"`), a second Codex account left commented, `[settings]` thresholds, and
  `limits_overlay = false` carrying a comment that points at the README overlay/ToS notice.

### B. `tok init` never clobbers an existing config

- If a file already exists at the resolved path, `tok init` refuses: it prints a message naming the
  path and that the config already exists, writes nothing, and exits `1`. A re-run can therefore
  never destroy a real config.

### C. Missing-config guidance

- When any config-loading command fails because the file does not exist (`NotFound`), the printed
  error gains a second line: `run \`tok init\` to create one`. (Other read failures — permissions,
  a bad parse — are unchanged; only the not-found case earns the hint.)

## Non-goals

- Interactive prompting / account discovery — the starter is a static, commented template the user
  edits. `tok validate` is the feedback loop.
- Overwriting or merging into an existing config (§B refuses instead).
- A `--force` flag — removing the file by hand is the explicit, unambiguous opt-out.

## Acceptance criteria

1. `tok init` at a `$TOKENOMICS_CONFIG` path creates the file (and any missing parent dir) and exits
   `0`; the result then passes `tok validate` cleanly (`no errors`). (A)
2. A second `tok init` at the same path exits `1`, names the path, says the config exists, and leaves
   the file untouched. (B)
3. The generated config contains `limits_overlay = false`. (A)
4. A config-loading command pointed at a nonexistent path suggests `` run `tok init` ``. (C)
5. `tokenomics.example.toml` is the single source of truth: `tok init` embeds it via `include_str!`,
   and the README example block matches it. (A)
6. `./check.sh` green.
