# Keeping overlay tokens warm (optional)

The opt-in overlay uses the account's Claude OAuth **access token**, read passively from
`<config_dir>/.credentials.json`. Tokenomics **never refreshes** that token itself — doing so would
POST to Anthropic and rewrite a Claude config dir, which is out of scope by design (see
`specs/007-…`). Instead it reuses the token Claude Code already maintains, and marks an account
`stale — open Claude to refresh` when the token has expired.

For a 24/7 overlay on an account you don't use interactively every few hours, you can keep its token
warm yourself. Two documented options (neither is required for v1):

## Option A — a `SessionStart` hook (warms the account when you use it)

Claude Code runs hooks on session start. A no-op session against an account refreshes its token as a
side effect. Add to that account's `~/.claude/settings.json` (i.e. under its `CLAUDE_CONFIG_DIR`):

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "true" } ] }
    ]
  }
}
```

Every time you start Claude Code for that account, its `.credentials.json` is refreshed, and the
collector's next overlay tick sees a warm token again.

## Option B — a periodic warm-up (cron / systemd timer)

Run a trivial non-interactive Claude Code invocation on a schedule, pinned to the account's config
dir, to trigger a token refresh:

```bash
# ~/.config/systemd/user/tok-warm-work.service  (paired with a .timer)
[Service]
Type=oneshot
Environment=CLAUDE_CONFIG_DIR=%h/.claude-acct/<name>
ExecStart=/usr/bin/claude -p "ping" --max-turns 0
```

```ini
# ~/.config/systemd/user/tok-warm-work.timer
[Timer]
OnCalendar=*-*-* 0/3:00:00      # every 3 hours
Persistent=true
[Install]
WantedBy=timers.target
```

Adjust the command to whatever your Claude Code version accepts for a minimal non-interactive run.
This warms the token without any interactive work.

## Why not automatic refresh?

- It would **write** to a Claude config dir (beyond the read-only reuse Tokenomics commits to).
- The headless refresh POST is WAF-fragile and ToS-gray.
- Passive reuse + a clear "open Claude to refresh" hint is safer and good enough: the accounts you
  actually use stay warm on their own.

If you want Tokenomics to attempt a best-effort refresh itself, that's a deliberate future opt-in —
ask, and it can be added behind an explicit `[settings]` flag.
