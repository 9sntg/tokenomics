# board.tape — reproducible demo recording for the tokenomics dashboard.
#
# Render with VHS (https://github.com/charmbracelet/vhs):
#     vhs docs/demo/board.tape
# Produces docs/demo/board.gif (referenced from the README).
#
# Deterministic by construction: fixed size, font, and theme so re-records diff cleanly.
# Point it at repo-local demo data so it never touches your real config/store — populate the
# demo store once with `TOKENOMICS_DB=docs/demo/demo.db tok collector --once` (or ship a fixture).

Output docs/demo/board.gif

Set Shell "bash"
Set FontSize 16
Set Width 1200
Set Height 720
Set Padding 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

# Silent setup: pin config + store to repo-local demo files (no real accounts touched).
Hide
Type "export TOKENOMICS_CONFIG=docs/demo/demo.toml TOKENOMICS_DB=docs/demo/demo.db" Enter
Show

# Launch the dashboard.
Type "tok" Sleep 500ms Enter

# Let the board render and settle.
Sleep 4s
Screenshot docs/demo/board.png

# Quit the TUI cleanly.
Type "q"
Sleep 1s
