//! Dashboard state + transitions + the precomputed per-account view rows (pure state machine).
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/tui/model.rs
//! Deps:    ratatui (Color), jiff; domain + format (display logic)
//! Tested:  inline `#[cfg(test)]` — update/selection, severity→color, NO_COLOR, view-row build,
//!          show-inactive toggle + tagged/dimmed inactive rows (spec 014), age-aware overlay hint
//!          (spec 015 §C)
//!
//! Key responsibilities:
//! - `App`: selection, help/quit/show-inactive flags, colour policy, and the precomputed
//!   `AccountView` rows.
//! - `update(Msg)`: fold Key / Data (store) / Tick / Resize; selection clamps to the row count.
//! - `build_account_view`: pure store-data → display-ready row (so `view` only lays out).
//!
//! Design constraints:
//! - Everything the view needs is precomputed here; colour is a pure function of state.
//! - `NO_COLOR` is honoured via `use_color` (resolved once at startup and threaded through).

use jiff::Timestamp;
use ratatui::style::Color;

use crate::domain::{Account, Limit, LimitKind, Provenance, Severity, UsageSnapshot};
use crate::format::{
    format_ago, format_ago_ms, format_cost, format_dollars, format_pct, format_reset,
    format_tokens, provenance_label, provenance_short, reset_expired, severity_label, RESET_DONE,
};
use crate::store::TokenStatus;
use crate::tui::keys::Action;

/// The local ccusage plane is flagged **stale** once its newest data is older than this many local
/// poll intervals (spec 011 §B) — the age segment then styles as a warning instead of dim.
const LOCAL_STALE_FACTOR: i64 = 2;
/// The collector is treated as **down** once its heartbeat is older than this many local poll
/// intervals (spec 011 §A) — beyond normal jitter, the writer has stopped.
const COLLECTOR_DOWN_FACTOR: i64 = 3;

/// A single-line gauge's display data (already resolved for the current colour policy). The parts
/// are kept structured (not a single pre-joined label) so each responsive tier can compose them:
/// FULL/COMPACT render `"[scope ]pct sev · resets reset"`, MICRO renders the columns separately.
#[derive(Debug, Clone)]
pub struct GaugeView {
    /// Fill fraction 0.0–1.0 (drives the proportional bar).
    pub ratio: f64,
    /// Utilization percent, e.g. `"76%"`.
    pub pct: String,
    /// Severity tier (drives the colour + the `● / ▲ / ✖` glyph + the `ok/warn/crit` word).
    pub severity: Severity,
    /// The reset countdown rendered verbatim from its source, e.g. `"in 2h 41m"` /
    /// `"waiting for reset"`.
    pub reset: Option<String>,
    /// Optional scope label (a model family) for a scoped weekly gauge, e.g. `"Fable"`.
    pub scope: Option<String>,
    /// Bar/label colour (already `Reset` when colour is disabled).
    pub color: Color,
    /// The limit's reset time has passed: the window reset, so the stored percent is history — the
    /// gauge renders dormant (`"waiting for reset"`) and never alarms (spec 012 §A).
    pub expired: bool,
}

impl GaugeView {
    /// Compose the roomy right-hand label: `"[scope ]pct sev[ · resets reset]"`, e.g.
    /// `"Fable 92% crit · resets in 5d 9h"`. Used by the FULL and COMPACT tiers.
    pub fn label(&self) -> String {
        // An expired gauge says only what is true: the window reset and we're waiting for fresh
        // evidence — no stale percent, no "ok" filler (spec 012 §A).
        if self.expired {
            return match &self.scope {
                Some(scope) => format!("{scope} {RESET_DONE}"),
                None => RESET_DONE.to_string(),
            };
        }
        let mut s = String::new();
        if let Some(scope) = &self.scope {
            s.push_str(scope);
            s.push(' ');
        }
        s.push_str(&self.pct);
        s.push(' ');
        s.push_str(severity_label(self.severity));
        if let Some(reset) = &self.reset {
            // A past reset reads standalone; a countdown takes the "resets" verb ("· resets in
            // 2h 41m"). Never "resets waiting for reset".
            s.push_str(if reset == RESET_DONE {
                " · "
            } else {
                " · resets "
            });
            s.push_str(reset);
        }
        s
    }

    /// The reset countdown without the leading `"in "` filler, for the tight MICRO columns
    /// (`"in 2h 41m"` → `"2h 41m"`). The time value itself is never reformatted (verbatim rule).
    pub fn reset_short(&self) -> Option<&str> {
        self.reset
            .as_deref()
            .map(|r| r.strip_prefix("in ").unwrap_or(r))
    }
}

/// A small coloured badge (e.g. the provenance tag).
#[derive(Debug, Clone)]
pub struct Badge {
    /// Badge text, e.g. `"derived"`.
    pub text: String,
    /// Abbreviated badge text for tight tiers, e.g. `"drv"`.
    pub short: String,
    /// Badge colour (already resolved for the colour policy).
    pub color: Color,
}

/// One account's display-ready row. `view` only lays these out — no computation.
#[derive(Debug, Clone)]
pub struct AccountView {
    /// Panel title, e.g. `"Personal [claude]"`.
    pub title: String,
    /// Accent colour for the panel border.
    pub accent: Color,
    /// The 5h session gauge, when a session limit exists.
    pub session: Option<GaugeView>,
    /// The weekly (all-models) gauge — `None` until the overlay lands ⇒ [`Self::weekly_hint`].
    pub weekly: Option<GaugeView>,
    /// The fallback note for a missing weekly gauge, honest about WHY it is missing: opt in, refresh
    /// the token, wait for the next overlay pass, or (spec 015 §C) age a past success that has gone
    /// silent. Owned because that last case is computed text, not a fixed literal.
    pub weekly_hint: String,
    /// The most-utilized per-model weekly gauge (e.g. `"Fable 92% crit"`), when the overlay
    /// reports a scoped weekly limit. `None` otherwise (no extra line is drawn).
    pub weekly_scoped: Option<GaugeView>,
    /// The single most-utilized gauge across session/weekly/scoped — the scariest number. Drives the
    /// one-line MICRO tier and the worst-offender banner so a glance always lands on the real risk.
    pub headline: Option<GaugeView>,
    /// A status note shown when there is no active session (e.g. `"idle"`, `"no data yet"`).
    pub status: Option<String>,
    /// The row's overall severity (drives the alert banner).
    pub severity: Severity,
    /// Mirrors `Account.active == false` (spec 014). An inactive row is only ever present in
    /// `App::rows` while `show_inactive` is on; it always stays excluded from the alert banner,
    /// the warn count, and the fleet reductions — see [`App::alert_count`] and `view::worst`.
    pub inactive: bool,
}

/// The fleet-wide usage line: the shared token / cost / burn figures shown **once** in the header
/// instead of repeated on every panel. On this deployment every account reads the same physical logs
/// (a shared `projects/` symlink — see spec 010), so a per-account meta line is the same number four
/// times; this collapses it to one row. Reducers take the representative usage and the *worst*
/// provenance / *oldest* refresh, so a single degraded account still surfaces.
// ponytail: the token/cost/burn reducers assume the shared-logs invariant (identical per-account
// usage). If accounts ever get their own real `projects/`, switch those reducers from max→sum for a
// true fleet total.
#[derive(Debug, Clone)]
pub struct FleetView {
    /// Shared total tokens, e.g. `"445.63M"` (or `"—"`).
    pub tokens: String,
    /// Notional cost, fully labeled: `"$382.65 (notional)"` (or `"—"`).
    pub cost_notional: String,
    /// Whole-dollar notional cost, self-labeled for tight widths: `"$382n"` (or `"—"`).
    pub cost_short: String,
    /// Fleet burn rate as tokens/hour, e.g. `"232.70M/h"` (or `None` when idle).
    pub burn_rate: Option<String>,
    /// The worst (most degraded) provenance across accounts.
    pub provenance: Option<Badge>,
    /// `"usage 12s ago"` — the local ccusage plane's age, from the *newest* snapshot across accounts.
    /// Present whenever any account has a snapshot, independent of the opt-in overlay (spec 011 §B).
    pub usage_age: Option<String>,
    /// Whether the local plane is stale (older than `LOCAL_STALE_FACTOR × poll_local_secs`) — the
    /// view then styles `usage_age` as a warning so a frozen number *looks* frozen.
    pub usage_stale: bool,
    /// `"limits 4m ago"` — the overlay/authoritative plane's age, from the *oldest* overlay refresh
    /// across accounts (most stale wins). Distinct from `usage_age` so the two planes never conflate.
    pub overlay_age: Option<String>,
}

/// One account's shared usage facts, extracted from its store reads — the raw numeric inputs to the
/// fleet reduction (kept numeric, unlike the pre-formatted `AccountView` strings, so they reduce).
#[derive(Debug, Clone, Copy, Default)]
pub struct AccountUsage {
    /// Total tokens in the account's latest snapshot.
    pub total_tokens: Option<u64>,
    /// Notional cost of the latest snapshot.
    pub cost_notional: Option<f64>,
    /// Active-window burn rate (ccusage tokens/minute), when a block is actively burning.
    pub tokens_per_minute: Option<f64>,
    /// The account's session-limit provenance (drives the fleet badge).
    pub provenance: Option<Provenance>,
    /// Epoch-millis of the account's last successful overlay fetch, if any (overlay-plane freshness).
    pub overlay_ms: Option<i64>,
    /// Epoch-millis of the account's latest snapshot, if any (local ccusage-plane freshness).
    pub collected_at_ms: Option<i64>,
}

/// One full store read: the per-account rows, the fleet-wide aggregate burn series, and the fleet
/// usage line (all read in a single tick so the header and the panels never mix data from different
/// reads).
#[derive(Debug, Default)]
pub struct Dashboard {
    /// The per-account display rows.
    pub rows: Vec<AccountView>,
    /// `Σ burn_tpm` per collection tick, oldest → newest (the header aggregate sparkline).
    pub aggregate_burn: Vec<u64>,
    /// The fleet-wide usage line, or `None` when no account has any data yet.
    pub fleet: Option<FleetView>,
    /// A collector-liveness alert (banner text) when the writer looks down/stalled, else `None`
    /// (spec 011 §A). Computed at the store-read site from the collector heartbeat age.
    pub collector_alert: Option<String>,
}

impl From<Vec<AccountView>> for Dashboard {
    /// Rows with no aggregate series or fleet line (used by tests and any rows-only refresh).
    fn from(rows: Vec<AccountView>) -> Self {
        Self {
            rows,
            aggregate_burn: Vec::new(),
            fleet: None,
            collector_alert: None,
        }
    }
}

/// A message folded by [`App::update`].
#[derive(Debug)]
pub enum Msg {
    /// A mapped key action.
    Key(Action),
    /// Fresh rows + aggregate series + fleet line read from the store. Boxed: it dwarfs the other
    /// variants, so a bare `Dashboard` would bloat every `Msg` (clippy `large_enum_variant`).
    Data(Box<Dashboard>),
    /// A periodic tick (clears the transient message).
    Tick,
    /// The terminal was resized (redraw happens naturally).
    Resize,
}

/// The dashboard state.
// Five independent UI flags (quit / help / reload / colour policy / show-inactive); grouping
// unrelated booleans into an enum would obscure, not clarify.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct App {
    /// Precomputed per-account rows — only the *visible* set (active, plus inactive when
    /// `show_inactive` is on). Selection/scrolling operate over this set directly (spec 014 §C).
    pub rows: Vec<AccountView>,
    /// Fleet-wide burn-rate series (`Σ burn_tpm` per tick, oldest → newest) for the header bar.
    pub aggregate_burn: Vec<u64>,
    /// The fleet-wide usage line (shared token/cost/burn/provenance/refresh), or `None` before data.
    pub fleet: Option<FleetView>,
    /// Collector-liveness alert (banner text) when the writer looks down/stalled, else `None`.
    pub collector_alert: Option<String>,
    /// Selected row index (clamped to `rows`).
    pub selected: usize,
    /// Set when the user asks to quit.
    pub should_quit: bool,
    /// Whether the help overlay is shown.
    pub show_help: bool,
    /// Set by `r` or `i`; the loop re-reads the store and clears it.
    pub reload_requested: bool,
    /// Colour policy (false when `NO_COLOR` is set).
    pub use_color: bool,
    /// Whether inactive accounts are included in `rows` (toggled by `i`; default off — spec 014 §C).
    pub show_inactive: bool,
    /// Number of visible accounts (for the header). Tracks `rows.len()` once data has landed; before
    /// the first store read it holds the caller's initial estimate.
    pub account_count: usize,
    /// A transient footer message.
    pub message: Option<String>,
}

impl App {
    /// Build an empty dashboard for an initial `account_count` (the caller passes the *visible*
    /// count for `show_inactive`'s default-off state, i.e. active accounts only).
    pub fn new(account_count: usize, use_color: bool) -> Self {
        Self {
            rows: Vec::new(),
            aggregate_burn: Vec::new(),
            fleet: None,
            collector_alert: None,
            selected: 0,
            should_quit: false,
            show_help: false,
            reload_requested: false,
            use_color,
            show_inactive: false,
            account_count,
            message: None,
        }
    }

    /// Fold a message into the state.
    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(action) => self.handle(action),
            Msg::Data(data) => {
                let data = *data;
                self.rows = data.rows;
                self.aggregate_burn = data.aggregate_burn;
                self.fleet = data.fleet;
                self.collector_alert = data.collector_alert;
                // Rows already reflect the current `show_inactive` filter (see `read_rows`), so the
                // header count tracks exactly what's on screen — including across a toggle.
                self.account_count = self.rows.len();
                self.clamp_selection();
            }
            Msg::Tick => self.message = None,
            Msg::Resize => {}
        }
    }

    fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Up => self.select_prev(),
            Action::Down => self.select_next(),
            Action::Refresh => {
                self.reload_requested = true;
                self.message = Some("refreshed".to_string());
            }
            Action::Help => self.show_help = !self.show_help,
            Action::ToggleInactive => {
                self.show_inactive = !self.show_inactive;
                self.reload_requested = true;
            }
        }
    }

    fn select_next(&mut self) {
        if !self.rows.is_empty() && self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    /// How many rows are at or above `Warn` (drives the alert banner). Inactive rows are excluded
    /// even when shown — peeking at a dead account must never raise the banner (spec 014 §C).
    pub fn alert_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| !r.inactive && r.severity != Severity::Ok)
            .count()
    }
}

/// Map a severity to its bar/text colour (a pure function of state).
pub fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Ok => Color::Green,
        Severity::Warn => Color::Yellow,
        Severity::Crit => Color::Red,
    }
}

/// Map a severity to an escalating status glyph (calm dot → caution triangle → cross). Paired with
/// the `ok/warn/crit` word so severity survives `NO_COLOR` — the glyph is never the sole cue.
pub fn severity_glyph(severity: Severity) -> &'static str {
    match severity {
        Severity::Ok => "●",
        Severity::Warn => "▲",
        Severity::Crit => "✖",
    }
}

/// Map a provenance to its badge colour.
pub fn provenance_color(source: Provenance) -> Color {
    match source {
        Provenance::Authoritative => Color::Green,
        Provenance::Derived => Color::Cyan,
        Provenance::Estimate => Color::DarkGray,
    }
}

/// Apply the colour policy: the real colour when enabled, else `Reset` (honours `NO_COLOR`).
pub fn resolve_color(use_color: bool, color: Color) -> Color {
    if use_color {
        color
    } else {
        Color::Reset
    }
}

/// The accent colour for an account's panel border (from its configured colour, else a default).
fn account_color(account: &Account, use_color: bool) -> Color {
    let color = account
        .color
        .as_deref()
        .and_then(parse_named_color)
        .unwrap_or(Color::Cyan);
    resolve_color(use_color, color)
}

/// Parse a named ratatui colour or a `#rrggbb` hex string.
fn parse_named_color(name: &str) -> Option<Color> {
    let lower = name.trim().to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    let color = match lower.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "white" => Color::White,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    };
    Some(color)
}

/// One account's stored data, as read by the TUI loop — the raw inputs to [`build_account_view`].
/// Bundled so the builder stays a small `(account, data, now, use_color)` call. All fields are
/// references or small `Copy` scalars, so the bundle itself is `Copy` (cheap to pass by value).
#[derive(Debug, Clone, Copy)]
pub struct AccountData<'a> {
    /// The last-good usage snapshot, if any.
    pub snapshot: Option<&'a UsageSnapshot>,
    /// The current merged limit set (session + any weekly rows).
    pub limits: &'a [Limit],
    /// The account's token freshness, if recorded.
    pub token_status: Option<TokenStatus>,
    /// Epoch-millis the overlay has been failing continuously since, if it is failing now.
    pub overlay_failing_since: Option<i64>,
    /// Epoch-millis of the account's last successful overlay fetch, if any — ages the "waiting for
    /// overlay" hint honestly once a past success has gone silent without a recorded failure
    /// (spec 015 §C; the same value `account_usage` reads for the fleet header).
    pub overlay_ms: Option<i64>,
}

/// How long an opted-in overlay must fail continuously before the row flags "check account". A live
/// account's 429s clear on the next success well within this; only a dead/blocked subscription (which
/// 429s every pass) stays failing long enough to trip it. Also the grace before a just-started
/// collector's first pending pass is called stalled.
const OVERLAY_STALL_MS: i64 = 15 * 60 * 1000;

/// The account-row status when the overlay's token is stale/absent.
///
/// macOS: "open Claude to refresh" cannot work — Claude Code refreshes into the Keychain, not
/// into the credentials file this reads, so the state would never clear (spec 014). Only
/// reachable on macOS if an account opted into the overlay, which is unsupported here anyway.
#[cfg(target_os = "macos")]
const STALE_TOKEN_STATUS: &str = "overlay unsupported on macOS (token is in the Keychain)";
#[cfg(not(target_os = "macos"))]
const STALE_TOKEN_STATUS: &str = "token stale — open Claude to refresh";

/// The weekly hint when the overlay is OFF.
///
/// macOS: enabling it changes nothing — Claude Code keeps the token in the Keychain and `tok`
/// deliberately does not read it (spec 014), so there is no token source to enable. Nudging
/// "enable overlay" would point the user at a feature that cannot work here AND that Anthropic's
/// terms do not permit.
#[cfg(target_os = "macos")]
const WEEKLY_OFF_HINT: &str = "n/a (unsupported on macOS)";
#[cfg(not(target_os = "macos"))]
const WEEKLY_OFF_HINT: &str = "n/a (enable overlay)";

/// The weekly hint when the overlay is ON but the token is stale/absent.
///
/// macOS: "open Claude" is wrong advice — Claude Code would refresh the token into the Keychain,
/// not into the credentials file this reads, so the state would never change (spec 014).
#[cfg(target_os = "macos")]
const WEEKLY_NO_TOKEN_HINT: &str = "n/a (no token on macOS — Keychain)";
#[cfg(not(target_os = "macos"))]
const WEEKLY_NO_TOKEN_HINT: &str = "n/a (token stale — open Claude)";

/// Build one account's display-ready row from its stored data. Pure (`now` injected).
pub fn build_account_view(
    account: &Account,
    data: AccountData<'_>,
    now: Timestamp,
    use_color: bool,
) -> AccountView {
    let AccountData {
        snapshot,
        limits,
        token_status,
        overlay_failing_since,
        overlay_ms,
    } = data;

    // Every gauge carries its OWN reset countdown (like Claude's /usage) — session, weekly-all, and
    // the per-model scoped weekly — so each line reads consistently: <pct> <sev> · resets <when>.
    let session_limit = limits.iter().find(|l| l.kind == LimitKind::Session);
    let session = session_limit.map(|limit| gauge_from_limit(limit, None, now, use_color));

    let weekly = limits
        .iter()
        .find(|l| l.kind == LimitKind::WeeklyAll)
        .map(|limit| gauge_from_limit(limit, None, now, use_color));
    let weekly_scoped = limits
        .iter()
        .filter(|l| l.kind == LimitKind::WeeklyScoped)
        .max_by(|a, b| a.utilization_pct.total_cmp(&b.utilization_pct))
        .map(|limit| gauge_from_limit(limit, limit.scope.as_deref(), now, use_color));

    // The headline is the single most-utilized gauge (session/weekly/scoped) — the scariest number.
    // The one-line MICRO tier and the worst-offender banner render this, so the eye lands on real risk.
    let headline = [&session, &weekly, &weekly_scoped]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.ratio.total_cmp(&b.ratio))
        .cloned();
    // An inactive account (shown only via `i`) renders its last-known rows dimmed, uniformly, so a
    // peeked-at dead account never competes visually with a live one's severity colour (spec 014 §C).
    let (session, weekly, weekly_scoped, headline) = if account.active {
        (session, weekly, weekly_scoped, headline)
    } else {
        let dim = |g: Option<GaugeView>| g.map(|g| dim_gauge(g, use_color));
        (dim(session), dim(weekly), dim(weekly_scoped), dim(headline))
    };

    // Token / cost / burn / provenance / refresh are identical across accounts (shared logs) and now
    // render once on the fleet header line — see `build_fleet_view` / `account_usage`. This per-account
    // row keeps only what differs between accounts: its gauges, status, and severity.
    let status = if token_status == Some(TokenStatus::Stale) {
        Some(STALE_TOKEN_STATUS.to_string())
    } else {
        match snapshot {
            None => Some("no data yet".to_string()),
            Some(s) if s.window.is_none() => Some("idle (no active block)".to_string()),
            Some(_) => None,
        }
    };
    // The row's severity is the worst of its NON-EXPIRED limits, so a critical scoped weekly lights
    // the banner even when the 5h window is calm — but a limit whose reset already passed is history
    // (the window reset) and must stop alarming the moment the countdown crosses zero (spec 012 §A).
    let severity = limits
        .iter()
        .filter(|l| !reset_expired(&l.resets_at, now))
        .map(|l| l.severity)
        .max()
        .unwrap_or(Severity::Ok);

    // The weekly fallback must not nudge "enable overlay" at an account that already opted in —
    // when the overlay is on but absent, the honest reasons are a stale token or a pending pass.
    // On macOS neither default nudge is true; see WEEKLY_OFF_HINT / WEEKLY_NO_TOKEN_HINT.
    let overlay_stalled = overlay_failing_since
        .is_some_and(|since| now.as_millisecond().saturating_sub(since) >= OVERLAY_STALL_MS);
    // A past overlay success that has since gone quiet with no *failed* attempt recorded (the
    // collector never retried it, e.g. spec 015's hot-reload gap) must not keep claiming "waiting" —
    // that reads as freshly pending when it is really stale. Aging the last success is the honest
    // signal; a genuinely never-succeeded account keeps the plain waiting hint.
    // Reached only via the `else if` below, i.e. once `overlay_stalled` is already known false —
    // that is the "stall flag hasn't tripped" condition from spec 015 §C.
    let overlay_silent_since_ms =
        overlay_ms.filter(|&ms| now.as_millisecond().saturating_sub(ms) > OVERLAY_STALL_MS);
    let weekly_hint = if !account.limits_overlay {
        WEEKLY_OFF_HINT.to_string()
    } else if token_status == Some(TokenStatus::Stale) {
        WEEKLY_NO_TOKEN_HINT.to_string()
    } else if overlay_stalled {
        // Warm token, opted in, but the overlay has failed every pass for a while — the account's
        // subscription is likely gone/blocked (a dead sub 429s /api/oauth/usage indefinitely).
        "n/a (overlay stalled — check account)".to_string()
    } else if let Some(ms) = overlay_silent_since_ms {
        format!("n/a (overlay silent {})", format_ago(ms, now))
    } else {
        "n/a (waiting for overlay)".to_string()
    };

    AccountView {
        title: account_title(account),
        accent: account_accent(account, use_color),
        session,
        weekly,
        weekly_hint,
        weekly_scoped,
        headline,
        status,
        severity,
        inactive: !account.active,
    }
}

/// Panel title, e.g. `"Personal [claude]"` — or `"Personal [claude] (inactive)"` when the account is
/// unsubscribed/paused. The tag lives in the title text (not just colour) so it survives `NO_COLOR`
/// and appears everywhere the title does (FULL/COMPACT/MICRO all render this same field).
fn account_title(account: &Account) -> String {
    let tag = if account.active { "" } else { " (inactive)" };
    format!("{} [{}]{tag}", account.label, account.provider)
}

/// The account's accent colour: its configured colour when active, or flattened to dim grey when
/// inactive (spec 014 §C, mirrors `dim_gauge`). Shared by `build_account_view` and `error_view` so
/// a store-read error never draws a stale-but-colourful accent for an account marked inactive.
fn account_accent(account: &Account, use_color: bool) -> Color {
    if account.active {
        account_color(account, use_color)
    } else {
        resolve_color(use_color, Color::DarkGray)
    }
}

/// Flatten a gauge's colour to dim grey — keeps its ratio/pct/severity/label data intact (still
/// accurate), just strips the colour emphasis (spec 014 §C: "dimmed", not hidden).
fn dim_gauge(mut g: GaugeView, use_color: bool) -> GaugeView {
    g.color = resolve_color(use_color, Color::DarkGray);
    g
}

/// Convert ccusage's tokens/minute burn rate to whole tokens/hour. Clamped non-negative and finite;
/// the `as u64` is bounded (any realistic rate fits u64 after the ×60), so truncation/sign loss are
/// unreachable — the `allow` documents that, matching the ccusage module's cast policy.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn tokens_per_hour(tokens_per_minute: f64) -> u64 {
    let per_hour = (tokens_per_minute * 60.0).round();
    if per_hour.is_finite() && per_hour >= 0.0 {
        per_hour as u64
    } else {
        0
    }
}

/// Build a gauge from a limit: keeps the parts structured (pct, severity, verbatim reset, scope) so
/// each tier composes its own label. `scope` names a model family (scoped weeklies); the reset is the
/// countdown to *this* limit's own reset, so every gauge shows the reset it belongs to.
fn gauge_from_limit(
    limit: &Limit,
    scope: Option<&str>,
    now: Timestamp,
    use_color: bool,
) -> GaugeView {
    // Reset already passed ⇒ the window reset and the stored percent is history, not state. Render
    // a dormant gauge — empty dim bar, no percent, Ok — that reads "waiting for reset" until fresh
    // evidence (a collect, or an overlay success after login) brings the new countdown (spec 012 §A).
    if reset_expired(&limit.resets_at, now) {
        return GaugeView {
            ratio: 0.0,
            pct: "—".to_string(),
            severity: Severity::Ok,
            reset: Some(RESET_DONE.to_string()),
            scope: scope.map(str::to_string),
            color: resolve_color(use_color, Color::DarkGray),
            expired: true,
        };
    }
    GaugeView {
        ratio: (limit.utilization_pct / 100.0).clamp(0.0, 1.0),
        pct: format_pct(limit.utilization_pct),
        severity: limit.severity,
        // An idle window carries no reset time (`resets_at: ""`) — no countdown, no "resets" tail.
        reset: (!limit.resets_at.is_empty()).then(|| format_reset(&limit.resets_at, now)),
        scope: scope.map(str::to_string),
        color: resolve_color(use_color, severity_color(limit.severity)),
        expired: false,
    }
}

/// Extract one account's shared usage facts from its store reads (pure) — the numeric inputs the
/// fleet reduction needs, mirroring how `build_account_view` picks the session provenance and the
/// active-window burn rate.
pub fn account_usage(
    snapshot: Option<&UsageSnapshot>,
    limits: &[Limit],
    overlay_ms: Option<i64>,
) -> AccountUsage {
    AccountUsage {
        total_tokens: snapshot.map(|s| s.total_tokens),
        cost_notional: snapshot.and_then(|s| s.cost_notional),
        tokens_per_minute: snapshot
            .and_then(|s| s.window.as_ref())
            .map(|w| w.tokens_per_minute)
            .filter(|&tpm| tpm > 0.0),
        provenance: limits
            .iter()
            .find(|l| l.kind == LimitKind::Session)
            .map(|l| l.source),
        overlay_ms,
        collected_at_ms: snapshot.map(|s| s.collected_at.as_millisecond()),
    }
}

/// Classify collector liveness from its heartbeat age (ms) against the local poll cadence, returning
/// the banner text when the writer looks down — else `None` (live). `None` age = the heartbeat row is
/// absent (never started against this store). Pure; the age is read at the store-read site.
pub fn collector_alert(heartbeat_age_ms: Option<i64>, poll_local_secs: u64) -> Option<String> {
    match heartbeat_age_ms {
        None => Some("collector not running — data frozen (start `tok collector`)".to_string()),
        Some(age_ms) => {
            let down_ms = i64::try_from(poll_local_secs.saturating_mul(1000))
                .unwrap_or(i64::MAX)
                .saturating_mul(COLLECTOR_DOWN_FACTOR);
            (age_ms > down_ms)
                .then(|| format!("collector stalled — last beat {}", format_ago_ms(age_ms)))
        }
    }
}

/// Degradation rank for the fleet reduction — higher is more degraded, so `max_by_key` picks the
/// worst provenance present (a single derived/estimate account is never hidden behind authoritative).
fn provenance_rank(source: Provenance) -> u8 {
    match source {
        Provenance::Authoritative => 0,
        Provenance::Derived => 1,
        Provenance::Estimate => 2,
    }
}

/// Reduce the per-account usage facts into the one fleet-wide line, or `None` when no account has any
/// data yet (so the header simply omits the line rather than showing a bare `"—"`). See [`FleetView`]
/// for why usage is a representative (max), not a sum.
pub fn build_fleet_view(
    usages: &[AccountUsage],
    now: Timestamp,
    use_color: bool,
    poll_local_secs: u64,
) -> Option<FleetView> {
    let total_tokens = usages.iter().filter_map(|u| u.total_tokens).max();
    let cost = usages
        .iter()
        .filter_map(|u| u.cost_notional)
        .max_by(f64::total_cmp);
    let tpm = usages
        .iter()
        .filter_map(|u| u.tokens_per_minute)
        .max_by(f64::total_cmp);
    let worst_prov = usages
        .iter()
        .filter_map(|u| u.provenance)
        .max_by_key(|p| provenance_rank(*p));
    let oldest_overlay = usages.iter().filter_map(|u| u.overlay_ms).min();
    // The local plane's freshness comes from the NEWEST snapshot across accounts (the most recent
    // collect); its age is shown even with the overlay off — the default deployment (spec 011 §B).
    let newest_local = usages.iter().filter_map(|u| u.collected_at_ms).max();

    if total_tokens.is_none()
        && cost.is_none()
        && tpm.is_none()
        && worst_prov.is_none()
        && oldest_overlay.is_none()
        && newest_local.is_none()
    {
        return None;
    }

    let stale_ms = i64::try_from(poll_local_secs.saturating_mul(1000))
        .unwrap_or(i64::MAX)
        .saturating_mul(LOCAL_STALE_FACTOR);
    let usage_stale = newest_local.is_some_and(|ms| now.as_millisecond() - ms > stale_ms);

    Some(FleetView {
        tokens: total_tokens.map_or_else(|| "—".to_string(), format_tokens),
        cost_notional: cost.map_or_else(|| "—".to_string(), format_cost),
        cost_short: cost.map_or_else(|| "—".to_string(), |c| format!("{}n", format_dollars(c))),
        burn_rate: tpm.map(|t| format!("{}/h", format_tokens(tokens_per_hour(t)))),
        provenance: worst_prov.map(|source| Badge {
            text: provenance_label(source).to_string(),
            short: provenance_short(source).to_string(),
            color: resolve_color(use_color, provenance_color(source)),
        }),
        usage_age: newest_local.map(|ms| format!("usage {}", format_ago(ms, now))),
        usage_stale,
        overlay_age: oldest_overlay.map(|ms| format!("limits {}", format_ago(ms, now))),
    })
}

/// A minimal row shown when an account's store read fails — so one bad read never blanks or crashes
/// the whole dashboard (the loop keeps the other accounts and retries next tick).
pub fn error_view(account: &Account, use_color: bool, message: &str) -> AccountView {
    AccountView {
        title: account_title(account),
        accent: account_accent(account, use_color),
        session: None,
        weekly: None,
        weekly_hint: "n/a".to_string(),
        weekly_scoped: None,
        headline: None,
        status: Some(format!("store read error: {message}")),
        severity: Severity::Ok,
        inactive: !account.active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Provider, Window};
    use std::path::PathBuf;

    fn view(severity: Severity) -> AccountView {
        AccountView {
            title: "T".to_string(),
            accent: Color::Cyan,
            session: None,
            weekly: None,
            weekly_hint: "n/a (enable overlay)".to_string(),
            weekly_scoped: None,
            headline: None,
            status: None,
            severity,
            inactive: false,
        }
    }

    #[test]
    fn selection_clamps_to_row_count() {
        let mut app = App::new(3, true);
        app.update(Msg::Data(Box::new(
            vec![view(Severity::Ok), view(Severity::Ok)].into(),
        )));
        app.update(Msg::Key(Action::Down));
        app.update(Msg::Key(Action::Down)); // would be index 2, clamps to 1
        assert_eq!(app.selected, 1);
        app.update(Msg::Key(Action::Up));
        assert_eq!(app.selected, 0);
        app.update(Msg::Key(Action::Up)); // saturating at 0
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn data_with_fewer_rows_reclamps_selection() {
        let mut app = App::new(3, true);
        app.update(Msg::Data(Box::new(vec![view(Severity::Ok); 3].into())));
        app.update(Msg::Key(Action::Down));
        app.update(Msg::Key(Action::Down));
        assert_eq!(app.selected, 2);
        app.update(Msg::Data(Box::new(vec![view(Severity::Ok)].into()))); // shrinks to 1
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn empty_data_reclamps_selection_and_renders_the_empty_state() {
        // Finding 7: an all-inactive / toggle-off transition delivers an EMPTY Msg::Data. Selection
        // (driven non-zero first) must reclamp to 0, further Up/Down must not panic on empty rows,
        // and the render must fall back to the empty-state placeholder rather than a tier layout.
        use crate::tui::view::render;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(3, true);
        app.update(Msg::Data(Box::new(vec![view(Severity::Ok); 3].into())));
        app.update(Msg::Key(Action::Down));
        app.update(Msg::Key(Action::Down));
        assert_eq!(app.selected, 2);

        // Everything drops out (e.g. all accounts inactive with show_inactive off).
        app.update(Msg::Data(Box::new(Vec::<AccountView>::new().into())));
        assert_eq!(app.selected, 0, "selection reclamps to 0 on empty data");

        // Further navigation on an empty board must be a no-op, never a panic/underflow.
        app.update(Msg::Key(Action::Down));
        app.update(Msg::Key(Action::Up));
        assert_eq!(app.selected, 0);

        // The render falls back to the empty-state placeholder rather than a tier layout.
        let mut term = Terminal::new(TestBackend::new(80, 20)).expect("backend");
        term.draw(|f| render(f, &app)).expect("draw");
        let buf = term.backend().buffer();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            text.contains("collecting"),
            "empty board must show the collecting… placeholder, got:\n{text}"
        );
    }

    #[test]
    fn quit_help_and_refresh_flags() {
        let mut app = App::new(1, true);
        app.update(Msg::Key(Action::Help));
        assert!(app.show_help);
        app.update(Msg::Key(Action::Refresh));
        assert!(app.reload_requested);
        app.update(Msg::Key(Action::Quit));
        assert!(app.should_quit);
    }

    #[test]
    fn toggle_inactive_flips_flag_and_requests_reload() {
        let mut app = App::new(1, true);
        assert!(!app.show_inactive, "hidden by default (spec 014 §C)");
        app.update(Msg::Key(Action::ToggleInactive));
        assert!(app.show_inactive);
        assert!(app.reload_requested, "toggling must re-read the store");
        app.reload_requested = false;
        app.update(Msg::Key(Action::ToggleInactive));
        assert!(!app.show_inactive, "pressing i again hides it");
        assert!(app.reload_requested);
    }

    #[test]
    fn alert_count_counts_warn_and_crit() {
        let mut app = App::new(3, true);
        app.update(Msg::Data(Box::new(
            vec![
                view(Severity::Ok),
                view(Severity::Warn),
                view(Severity::Crit),
            ]
            .into(),
        )));
        assert_eq!(app.alert_count(), 2);
    }

    #[test]
    fn alert_count_ignores_inactive_rows_even_at_crit() {
        // A crit-level stored limit on an inactive (peeked-at) account must never raise the banner
        // or the warn count (spec 014 §C, acceptance criteria 3–4).
        let mut app = App::new(2, true);
        let mut inactive_crit = view(Severity::Crit);
        inactive_crit.inactive = true;
        app.update(Msg::Data(Box::new(
            vec![view(Severity::Warn), inactive_crit].into(),
        )));
        assert_eq!(app.alert_count(), 1, "only the active warn row counts");
    }

    #[test]
    fn account_count_tracks_visible_rows_across_data() {
        // account_count starts at the caller's initial (active-only) estimate, then tracks
        // `rows.len()` as data lands — including a shrink/grow across a show_inactive toggle.
        let mut app = App::new(1, true);
        assert_eq!(app.account_count, 1);
        app.update(Msg::Data(Box::new(
            vec![view(Severity::Ok), view(Severity::Ok)].into(),
        )));
        assert_eq!(app.account_count, 2);
    }

    #[test]
    fn severity_maps_to_stable_colors() {
        assert_eq!(severity_color(Severity::Ok), Color::Green);
        assert_eq!(severity_color(Severity::Warn), Color::Yellow);
        assert_eq!(severity_color(Severity::Crit), Color::Red);
    }

    #[test]
    fn no_color_resolves_to_reset() {
        assert_eq!(resolve_color(true, Color::Red), Color::Red);
        assert_eq!(resolve_color(false, Color::Red), Color::Reset);
    }

    #[test]
    fn parses_named_and_hex_colors() {
        assert_eq!(parse_named_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_named_color("  LightBlue "), Some(Color::LightBlue));
        assert_eq!(parse_named_color("#00ffcc"), Some(Color::Rgb(0, 255, 204)));
        assert_eq!(parse_named_color("burple"), None);
    }

    fn account() -> Account {
        Account {
            id: "personal".to_string(),
            label: "Personal".to_string(),
            provider: Provider::Claude,
            config_dir: PathBuf::from("/tmp"),
            color: Some("cyan".to_string()),
            active: true,
            limits_overlay: false,
        }
    }

    #[test]
    fn build_view_from_snapshot_and_limit() {
        let snapshot = UsageSnapshot {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            collected_at: "2026-07-04T10:00:00Z".parse().unwrap(),
            input: 1,
            output: 1,
            cache_read: 1,
            cache_creation: 1,
            total_tokens: 1_234_567,
            cost_notional: Some(1.7),
            window: None,
        };
        let limit = Limit {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            kind: LimitKind::Session,
            scope: None,
            utilization_pct: 76.0,
            resets_at: "2026-07-04T12:00:00Z".to_string(),
            severity: Severity::Warn,
            source: Provenance::Derived,
        };
        let now: Timestamp = "2026-07-04T10:49:00Z".parse().unwrap();
        let limits = [limit];
        let data = AccountData {
            snapshot: Some(&snapshot),
            limits: &limits,
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: None,
        };
        let row = build_account_view(&account(), data, now, true);

        assert_eq!(row.title, "Personal [claude]");
        let session = row.session.expect("session gauge");
        assert!((session.ratio - 0.76).abs() < 1e-9);
        // The composed 5h label carries its own reset countdown.
        assert_eq!(session.label(), "76% warn · resets in 1h 11m");
        // The tight MICRO tier strips the leading "in " (but never reformats the time value).
        assert_eq!(session.reset_short(), Some("1h 11m"));
        // With only a session limit, the headline is that session.
        assert_eq!(row.headline.expect("headline").label(), session.label());
        assert_eq!(row.status.as_deref(), Some("idle (no active block)"));
        assert_eq!(row.severity, Severity::Warn);
        // The shared token/cost/provenance figures now come off `account_usage` (fleet line), not the
        // per-account row. No active window ⇒ no burn rate.
        let usage = account_usage(Some(&snapshot), &limits, None);
        assert_eq!(usage.total_tokens, Some(1_234_567));
        assert_eq!(usage.cost_notional, Some(1.7));
        assert_eq!(usage.provenance, Some(Provenance::Derived));
        assert_eq!(usage.tokens_per_minute, None);
    }

    #[test]
    fn inactive_account_view_is_tagged_and_dimmed() {
        // Spec 014 §C: when shown (via `i`), an inactive account's title carries an "(inactive)" tag
        // and every gauge/accent colour is flattened to dim grey — but its numbers stay accurate.
        let inactive = Account {
            active: false,
            ..account()
        };
        let limit = Limit {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            kind: LimitKind::Session,
            scope: None,
            utilization_pct: 91.0,
            resets_at: "2026-07-04T12:00:00Z".to_string(),
            severity: Severity::Crit,
            source: Provenance::Derived,
        };
        let now: Timestamp = "2026-07-04T10:49:00Z".parse().unwrap();
        let limits = [limit];
        let row = build_account_view(&inactive, data_from(&limits), now, true);

        assert_eq!(row.title, "Personal [claude] (inactive)");
        assert!(row.inactive);
        assert_eq!(row.accent, Color::DarkGray);
        let session = row.session.expect("session gauge");
        // The percent/severity data is untouched (still an accurate peek)...
        assert_eq!(session.pct, "91%");
        assert_eq!(session.severity, Severity::Crit);
        // ...but the colour is flattened to dim grey rather than the crit-red it would otherwise be.
        assert_eq!(session.color, Color::DarkGray);
        assert_eq!(row.headline.expect("headline").color, Color::DarkGray);

        // NO_COLOR: the tag stays (it's structural, in the title text) even though colour collapses.
        let mono = build_account_view(&inactive, data_from(&limits), now, false);
        assert_eq!(mono.title, "Personal [claude] (inactive)");
        assert_eq!(mono.accent, Color::Reset);
    }

    #[test]
    fn fleet_view_formats_tokens_per_hour_from_window_burn_rate() {
        // 4520.0 tok/min × 60 = 271_200 tok/hour ⇒ "271.2K/h" (same basis/formatter as `tokens`).
        let snapshot = UsageSnapshot {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            collected_at: "2026-07-04T10:00:00Z".parse().unwrap(),
            input: 1,
            output: 1,
            cache_read: 1,
            cache_creation: 1,
            total_tokens: 5_000_000,
            cost_notional: Some(1.7),
            window: Some(Window {
                start: "2026-07-04T07:00:00Z".parse().unwrap(),
                end: "2026-07-04T12:00:00Z".parse().unwrap(),
                remaining_minutes: Some(90),
                tokens_per_minute: 4520.0,
                cost_per_hour: 6.7,
            }),
        };
        let now: Timestamp = "2026-07-04T10:49:00Z".parse().unwrap();
        let usage = account_usage(Some(&snapshot), &[], None);
        assert_eq!(usage.tokens_per_minute, Some(4520.0));
        let fleet = build_fleet_view(&[usage], now, true, 20).expect("fleet");
        assert_eq!(fleet.burn_rate.as_deref(), Some("271.2K/h"));
    }

    #[test]
    fn fleet_view_shows_local_usage_age_even_with_overlay_off() {
        // Default deployment: overlay off (overlay_ms None), but a fresh snapshot ⇒ a local age still
        // shows, and past 2× the cadence it is flagged stale (so it can be styled as a warning).
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let fresh: Timestamp = "2026-07-04T11:59:48Z".parse().unwrap(); // 12s ago
        let usage = account_usage(
            Some(&UsageSnapshot {
                account_id: "a".to_string(),
                provider: Provider::Claude,
                collected_at: fresh,
                input: 1,
                output: 1,
                cache_read: 1,
                cache_creation: 1,
                total_tokens: 4,
                cost_notional: Some(0.1),
                window: None,
            }),
            &[],
            None,
        );
        let fleet = build_fleet_view(&[usage], now, true, 20).expect("fleet");
        assert_eq!(fleet.usage_age.as_deref(), Some("usage 12s ago"));
        assert!(!fleet.usage_stale, "12s < 2×20s is fresh");
        assert_eq!(fleet.overlay_age, None, "overlay off ⇒ no limits age");

        // A snapshot 90s old (> 2×20s) is stale.
        let stale: Timestamp = "2026-07-04T11:58:30Z".parse().unwrap();
        let mut u = usage;
        u.collected_at_ms = Some(stale.as_millisecond());
        let fleet = build_fleet_view(&[u], now, true, 20).expect("fleet");
        assert!(fleet.usage_stale, "90s > 2×20s is stale");
    }

    #[test]
    fn collector_alert_flags_down_and_stalled_but_not_live() {
        // Never started (no heartbeat row) ⇒ loud "not running" banner.
        assert_eq!(
            collector_alert(None, 10).as_deref(),
            Some("collector not running — data frozen (start `tok collector`)")
        );
        // Fresh beat (5s < 3×10s) ⇒ no alert.
        assert!(collector_alert(Some(5_000), 10).is_none());
        // Stalled beat (90s > 3×10s) ⇒ "stalled — last beat …".
        assert_eq!(
            collector_alert(Some(90_000), 10).as_deref(),
            Some("collector stalled — last beat 1m ago")
        );
        // Exact strict-greater-than boundary: down_ms = 3 × 10s = 30_000ms. Equal is NOT stalled
        // (the `>` is strict, so the boundary tick itself stays live)...
        assert!(collector_alert(Some(30_000), 10).is_none());
        // ...and one ms past it trips the stalled banner.
        assert!(collector_alert(Some(30_001), 10).is_some());
    }

    #[test]
    fn tokens_per_hour_clamps_non_finite_and_negative() {
        assert_eq!(tokens_per_hour(1000.0), 60_000);
        assert_eq!(tokens_per_hour(0.0), 0);
        assert_eq!(tokens_per_hour(-5.0), 0);
        assert_eq!(tokens_per_hour(f64::NAN), 0);
        assert_eq!(tokens_per_hour(f64::INFINITY), 0);
    }

    /// Minimal `AccountData` (no snapshot/token) for limit-focused build tests.
    fn data_from(limits: &[Limit]) -> AccountData<'_> {
        AccountData {
            snapshot: None,
            limits,
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: None,
        }
    }

    fn authoritative(kind: LimitKind, scope: Option<&str>, pct: f64, sev: Severity) -> Limit {
        Limit {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            kind,
            scope: scope.map(str::to_string),
            utilization_pct: pct,
            resets_at: "2026-07-10T03:00:00.44+00:00".to_string(),
            severity: sev,
            source: Provenance::Authoritative,
        }
    }

    #[test]
    fn build_view_surfaces_weekly_and_scoped_gauges_and_worst_severity() {
        let now: Timestamp = "2026-07-04T10:00:00Z".parse().unwrap();
        let limits = vec![
            authoritative(LimitKind::Session, None, 29.0, Severity::Ok),
            authoritative(LimitKind::WeeklyAll, None, 78.0, Severity::Warn),
            authoritative(LimitKind::WeeklyScoped, Some("Fable"), 92.0, Severity::Crit),
        ];
        let row = build_account_view(&account(), data_from(&limits), now, true);

        // Every gauge carries its own reset — session, weekly-all, and the scoped weekly alike.
        assert!(row
            .session
            .expect("session")
            .label()
            .starts_with("29% ok · resets "));
        assert!(row
            .weekly
            .expect("weekly-all")
            .label()
            .starts_with("78% warn · resets "));
        let scoped = row.weekly_scoped.expect("scoped weekly");
        assert!(scoped.label().starts_with("Fable 92% crit · resets "));
        assert!((scoped.ratio - 0.92).abs() < 1e-9);
        // The headline is the scariest gauge — the 92% crit scoped weekly, not the calm 29% session.
        assert!(row
            .headline
            .expect("headline")
            .label()
            .starts_with("Fable 92% crit"));
        // Worst-of-all: the critical scoped weekly wins even though the 5h session is Ok.
        assert_eq!(row.severity, Severity::Crit);
    }

    #[test]
    fn expired_limit_renders_waiting_for_reset_and_stops_alarming() {
        // A crit weekly whose reset passed a day ago: the window reset, so the stored 100% is
        // history — the gauge goes dormant and the row leaves the banner (spec 012 §A).
        let now: Timestamp = "2026-07-08T10:00:00Z".parse().unwrap();
        let mut past = authoritative(LimitKind::WeeklyAll, None, 100.0, Severity::Crit);
        past.resets_at = "2026-07-07T09:00:00Z".to_string();
        let limits = vec![past];
        let row = build_account_view(&account(), data_from(&limits), now, true);
        let weekly = row.weekly.expect("weekly");
        assert!(weekly.expired);
        assert_eq!(weekly.label(), "waiting for reset");
        assert_eq!(weekly.pct, "—");
        assert!(weekly.ratio.abs() < 1e-9, "dormant bar");
        assert_eq!(weekly.severity, Severity::Ok);
        assert_eq!(row.severity, Severity::Ok, "expired crit must not alarm");
    }

    #[test]
    fn non_expired_severity_still_wins_and_scoped_expired_keeps_its_scope() {
        let now: Timestamp = "2026-07-08T10:00:00Z".parse().unwrap();
        let mut expired = authoritative(LimitKind::WeeklyAll, None, 100.0, Severity::Crit);
        expired.resets_at = "2026-07-07T09:00:00Z".to_string();
        // The session fixture resets 2026-07-10 — still live, so its warn drives the row.
        let live = authoritative(LimitKind::Session, None, 80.0, Severity::Warn);
        let row = build_account_view(&account(), data_from(&[live, expired]), now, true);
        assert_eq!(row.severity, Severity::Warn);

        let mut scoped =
            authoritative(LimitKind::WeeklyScoped, Some("Fable"), 99.0, Severity::Crit);
        scoped.resets_at = "2026-07-07T09:00:00Z".to_string();
        let row = build_account_view(&account(), data_from(&[scoped]), now, true);
        assert_eq!(
            row.weekly_scoped.expect("scoped").label(),
            "Fable waiting for reset"
        );
    }

    #[test]
    fn build_view_picks_the_most_utilized_scoped_weekly() {
        let now: Timestamp = "2026-07-04T10:00:00Z".parse().unwrap();
        let limits = vec![
            authoritative(LimitKind::WeeklyScoped, Some("Sonnet"), 40.0, Severity::Ok),
            authoritative(LimitKind::WeeklyScoped, Some("Fable"), 92.0, Severity::Crit),
        ];
        let row = build_account_view(&account(), data_from(&limits), now, true);
        assert!(row
            .weekly_scoped
            .expect("scoped")
            .label()
            .starts_with("Fable 92% crit · resets "));
    }

    #[test]
    fn weekly_hint_when_the_overlay_is_off_never_nudges_toward_something_unusable() {
        // Spec 014. These two arms were the only unpinned hints, which is why changing them broke
        // nothing — the gap this test closes.
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let off = Account {
            active: true,
            limits_overlay: false,
            ..account()
        };
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: None,
        };
        let row = build_account_view(&off, data, now, true);
        assert_eq!(row.weekly_hint, WEEKLY_OFF_HINT);

        #[cfg(target_os = "macos")]
        {
            // Enabling it changes nothing here (no token source) and the overlay is not permitted
            // anyway — the hint must not send the user after it.
            assert!(!row.weekly_hint.contains("enable"), "{}", row.weekly_hint);
            assert!(row.weekly_hint.contains("macOS"), "{}", row.weekly_hint);
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(row.weekly_hint, "n/a (enable overlay)");
    }

    #[test]
    fn a_stale_token_never_advises_something_that_cannot_help() {
        // On macOS "open Claude to refresh" is false advice: Claude refreshes into the Keychain,
        // not into the file this reads, so the state would never clear (spec 014).
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: Some(TokenStatus::Stale),
            overlay_failing_since: None,
            overlay_ms: None,
        };
        let row = build_account_view(&opted_in, data, now, true);
        assert_eq!(row.weekly_hint, WEEKLY_NO_TOKEN_HINT);
        assert_eq!(row.status.as_deref(), Some(STALE_TOKEN_STATUS));

        #[cfg(target_os = "macos")]
        {
            assert!(!row.weekly_hint.contains("open Claude"));
            assert!(!row.status.unwrap_or_default().contains("open Claude"));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(row.weekly_hint, "n/a (token stale — open Claude)");
            assert_eq!(
                row.status.as_deref(),
                Some("token stale — open Claude to refresh")
            );
        }
    }

    #[test]
    fn sustained_overlay_failure_flags_check_account_but_a_recent_one_still_waits() {
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let mk = |failing_since: Option<Timestamp>| AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: failing_since.map(jiff::Timestamp::as_millisecond),
            overlay_ms: None,
        };

        // Failing for 20m (past the 15m stall threshold) ⇒ the honest "check account" flag.
        let stalled: Timestamp = "2026-07-04T11:40:00Z".parse().unwrap();
        let row = build_account_view(&opted_in, mk(Some(stalled)), now, true);
        assert_eq!(row.weekly_hint, "n/a (overlay stalled — check account)");

        // A 5m-old failure is within grace (a live account's transient 429) ⇒ still just waiting.
        let recent: Timestamp = "2026-07-04T11:55:00Z".parse().unwrap();
        let row = build_account_view(&opted_in, mk(Some(recent)), now, true);
        assert_eq!(row.weekly_hint, "n/a (waiting for overlay)");
    }

    #[test]
    fn overlay_silent_after_past_success_shows_aged_hint() {
        // The overlay succeeded once, long enough ago to cross the stall threshold, but no failed
        // attempt has been recorded since (e.g. the collector never got around to retrying it) —
        // the honest hint ages the past success instead of pretending it is freshly pending.
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let last_success: Timestamp = "2026-07-04T11:30:00Z".parse().unwrap(); // 30m ago > 15m stall
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: Some(last_success.as_millisecond()),
        };
        let row = build_account_view(&opted_in, data, now, true);
        assert_eq!(row.weekly_hint, "n/a (overlay silent 30m ago)");
    }

    #[test]
    fn overlay_never_succeeded_keeps_waiting_hint() {
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: None,
        };
        let row = build_account_view(&opted_in, data, now, true);
        assert_eq!(row.weekly_hint, "n/a (waiting for overlay)");
    }

    #[test]
    fn overlay_fresh_success_keeps_waiting_hint() {
        // A success inside the stall window is not stale enough to age — same honest "waiting" as
        // a never-succeeded account, since nothing is actually wrong yet.
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let recent_success: Timestamp = "2026-07-04T11:58:00Z".parse().unwrap(); // 2m ago < 15m
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: None,
            overlay_ms: Some(recent_success.as_millisecond()),
        };
        let row = build_account_view(&opted_in, data, now, true);
        assert_eq!(row.weekly_hint, "n/a (waiting for overlay)");
    }

    #[test]
    fn overlay_stalled_flag_wins_over_silent_hint() {
        // Both a tripped stall flag AND a stale past success are present — the stalled branch still
        // wins (a dead/blocked subscription is worse, more actionable news than "gone quiet").
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let opted_in = Account {
            active: true,
            limits_overlay: true,
            ..account()
        };
        let stalled_since: Timestamp = "2026-07-04T11:40:00Z".parse().unwrap();
        let last_success: Timestamp = "2026-07-04T10:00:00Z".parse().unwrap();
        let data = AccountData {
            snapshot: None,
            limits: &[],
            token_status: None,
            overlay_failing_since: Some(stalled_since.as_millisecond()),
            overlay_ms: Some(last_success.as_millisecond()),
        };
        let row = build_account_view(&opted_in, data, now, true);
        assert_eq!(row.weekly_hint, "n/a (overlay stalled — check account)");
    }

    #[test]
    fn fleet_view_reports_overlay_refresh_age_scoped_as_limits() {
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        let two_min_ago: Timestamp = "2026-07-04T11:58:00Z".parse().unwrap();
        let usage = account_usage(None, &[], Some(two_min_ago.as_millisecond()));
        let fleet = build_fleet_view(&[usage], now, true, 20).expect("fleet");
        // The overlay age is scoped "limits …" so it can never imply the local numbers are fresh.
        assert_eq!(fleet.overlay_age.as_deref(), Some("limits 2m ago"));
        assert_eq!(fleet.usage_age, None, "no snapshot ⇒ no local age");

        // No recorded overlay success and no snapshot anywhere ⇒ no fleet line.
        assert!(build_fleet_view(&[account_usage(None, &[], None)], now, true, 20).is_none());
    }

    #[test]
    fn account_usage_extracts_shared_facts() {
        let snapshot = UsageSnapshot {
            account_id: "personal".to_string(),
            provider: Provider::Claude,
            collected_at: "2026-07-04T10:00:00Z".parse().unwrap(),
            input: 1,
            output: 1,
            cache_read: 1,
            cache_creation: 1,
            total_tokens: 280_000_000,
            cost_notional: Some(335.95),
            window: Some(Window {
                start: "2026-07-04T07:00:00Z".parse().unwrap(),
                end: "2026-07-04T12:00:00Z".parse().unwrap(),
                remaining_minutes: Some(90),
                tokens_per_minute: 4520.0,
                cost_per_hour: 6.7,
            }),
        };
        let limits = vec![authoritative(LimitKind::Session, None, 42.0, Severity::Ok)];
        let u = account_usage(Some(&snapshot), &limits, Some(1_720_000_000_000));
        assert_eq!(u.total_tokens, Some(280_000_000));
        assert_eq!(u.cost_notional, Some(335.95));
        assert_eq!(u.tokens_per_minute, Some(4520.0));
        assert_eq!(u.provenance, Some(Provenance::Authoritative));
        assert_eq!(u.overlay_ms, Some(1_720_000_000_000));
        assert_eq!(
            u.collected_at_ms,
            Some(snapshot.collected_at.as_millisecond())
        );

        // No snapshot / no session limit ⇒ all facts absent (the error-isolation path).
        let empty = account_usage(None, &[], None);
        assert_eq!(empty.total_tokens, None);
        assert_eq!(empty.tokens_per_minute, None);
        assert_eq!(empty.provenance, None);
        assert_eq!(empty.collected_at_ms, None);
    }

    #[test]
    fn fleet_view_reduces_representative_usage_worst_provenance_oldest_refresh() {
        let now: Timestamp = "2026-07-04T12:00:00Z".parse().unwrap();
        // Two accounts reading the same shared logs (identical usage), collected a tick apart, and
        // one whose overlay has degraded to derived.
        let a = AccountUsage {
            total_tokens: Some(445_630_000),
            cost_notional: Some(382.65),
            tokens_per_minute: Some(3_878_000.0),
            provenance: Some(Provenance::Authoritative),
            overlay_ms: Some(
                "2026-07-04T11:58:00Z"
                    .parse::<Timestamp>()
                    .unwrap()
                    .as_millisecond(),
            ),
            collected_at_ms: Some(
                "2026-07-04T11:59:00Z"
                    .parse::<Timestamp>()
                    .unwrap()
                    .as_millisecond(),
            ),
        };
        let b = AccountUsage {
            provenance: Some(Provenance::Derived),
            overlay_ms: Some(
                "2026-07-04T11:50:00Z"
                    .parse::<Timestamp>()
                    .unwrap()
                    .as_millisecond(),
            ),
            ..a
        };
        let fleet = build_fleet_view(&[a, b], now, true, 20).expect("some fleet");
        // Representative (identical) usage — never summed.
        assert_eq!(fleet.tokens, "445.63M");
        assert_eq!(fleet.cost_notional, "$382.65 (notional)");
        assert_eq!(fleet.cost_short, "$382n");
        assert_eq!(fleet.burn_rate.as_deref(), Some("232.68M/h")); // 3.878M × 60 = 232.68M
                                                                   // Worst provenance (derived beats authoritative), oldest overlay (11:50 = "limits 10m ago"),
                                                                   // and the NEWEST local snapshot (11:59 = "usage 1m ago").
        assert_eq!(fleet.provenance.expect("badge").text, "derived");
        assert_eq!(fleet.overlay_age.as_deref(), Some("limits 10m ago"));
        assert_eq!(fleet.usage_age.as_deref(), Some("usage 1m ago"));

        // No account has any data ⇒ no fleet line at all.
        assert!(build_fleet_view(&[AccountUsage::default()], now, true, 20).is_none());
    }
}
