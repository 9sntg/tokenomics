# Running the collector

`tok collector` is the background writer: it polls every configured account on the local cadence
(`[settings].poll_local_secs`) and writes snapshots + derived limits to the SQLite store. The TUI
(and `tok collector --once`) read that store. It is ToS-safe (local ccusage only) until you opt an
account into the overlay.

- **One-shot** (cron-friendly): `tok collector --once` — collect once, persist, print a read-back
  summary, exit.
- **Daemon** (24/7): `tok collector` — loop until `SIGINT`/`SIGTERM`, writing a `heartbeat` each tick.

## systemd `--user` unit (optional; not auto-installed)

Run the daemon as a user service so it starts with your session and restarts on failure. Adjust the
`ExecStart` path to wherever you installed the `tok` binary (e.g. `~/.local/bin/tok` or the built
`target/release/tok`).

```ini
# ~/.config/systemd/user/tokenomics-collector.service
[Unit]
Description=Tokenomics collector (LLM subscription usage → local store)
After=default.target

[Service]
Type=simple
ExecStart=%h/.local/bin/tok collector
Restart=on-failure
RestartSec=10
# tok reads ~/.config/tokenomics/tokenomics.toml and writes the XDG data-dir store.

[Install]
WantedBy=default.target
```

Install and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now tokenomics-collector.service
systemctl --user status tokenomics-collector.service
journalctl --user -u tokenomics-collector.service -f
```

To keep it running after you log out (headless WSL/servers):

```bash
loginctl enable-linger "$USER"
```

Stop / disable:

```bash
systemctl --user disable --now tokenomics-collector.service
```
