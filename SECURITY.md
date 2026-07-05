# Security Policy

## Reporting a vulnerability

Please report security issues privately via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
(the **Report a vulnerability** button under this repository's **Security** tab). Do not open a
public issue for a suspected vulnerability.

This is a passively maintained personal project: expect a **best-effort** response, not a guaranteed
SLA. Please give reasonable time for a fix before any public disclosure.

## Threat model notes

`tok` is a local-first, single-user tool. A few properties are worth stating explicitly:

- **The limits overlay is opt-in and off by default.** Only accounts that set
  `limits_overlay = true` ever make a network request; with it off, the tool is entirely offline.
- **Tokens are never logged, printed, or stored.** When the overlay is enabled, `tok` reads the
  OAuth access token Claude Code already maintains, uses it for a single authenticated request, and
  never persists or emits it. External calls go over HTTPS (rustls) with timeouts.
- **The store is a local SQLite database** (`~/.local/share/tokenomics/tokenomics.db`), written and
  read only by this tool on your machine. It contains token counts and window utilization — no
  credentials.
- **Subprocesses are invoked by explicit argv** (never a shell), and every external call has a
  timeout.

See [`CLAUDE.md`](CLAUDE.md) and [`RESEARCH.md`](RESEARCH.md) for the full two-plane design (ToS-safe
local core vs. the opt-in overlay).
