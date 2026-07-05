//! The opt-in `/api/oauth/usage` overlay: parse authoritative limits + a 429 backoff + the endpoint.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/providers/claude/overlay.rs
//! Deps:    serde_json, reqwest (rustls), async-trait; domain + format
//! Tested:  inline `#[cfg(test)]` — parse (200/malformed/scoped), backoff, canned endpoint
//!
//! Key responsibilities:
//! - `parse_oauth_usage` (pure): body → `Vec<Limit> { source = Authoritative }`, `resets_at` verbatim.
//! - `next_backoff` (pure): grow on 429 (no Retry-After), reset on success, capped.
//! - `UsageEndpoint` trait + `HttpUsageEndpoint` (reqwest) — the only network touch; opt-in gated.
//!
//! Design constraints:
//! - Undocumented, 429-prone, ToS-gray → strictly opt-in and MUST degrade to derived (caller's job).
//! - The bearer token never appears in a log or error (reqwest errors carry the URL, not headers).

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::{Limit, LimitKind, Provenance, Provider};
use crate::error::{AppError, AppResult};
use crate::format::severity_for;

const OVERLAY_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// The client identity the endpoint expects (Claude Code, not the SDK). May need bumping over time.
const USER_AGENT: &str = "claude-code/1.0.0";
const OVERLAY_TIMEOUT_SECS: u64 = 10;

/// The overlay response. The current endpoint carries a canonical `limits[]` array (session +
/// weekly-all + per-model weekly-scoped); the flat `five_hour`/`seven_day*` fields are the legacy
/// shape kept as a fallback. Unknown top-level fields (there are many) are ignored by serde.
#[derive(Debug, Deserialize)]
struct OauthUsage {
    /// Canonical, forward-compatible list (preferred when non-empty).
    #[serde(default)]
    limits: Vec<UsageLimit>,
    // --- legacy flat windows (fallback only) ---
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    seven_day_opus: Option<UsageWindow>,
    seven_day_sonnet: Option<UsageWindow>,
}

/// One entry of the canonical `limits[]` array.
#[derive(Debug, Deserialize)]
struct UsageLimit {
    /// `"session"` | `"weekly_all"` | `"weekly_scoped"` (unknown kinds are skipped).
    kind: String,
    #[serde(
        alias = "utilization",
        alias = "used_percentage",
        alias = "used_percent"
    )]
    percent: f64,
    /// `null` when the window is idle (e.g. no active 5h session) — mapped to `""` (no countdown).
    #[serde(default, alias = "reset_at")]
    resets_at: Option<String>,
    /// Present for `weekly_scoped` (the per-model sub-limit); its model display name is the scope.
    #[serde(default)]
    scope: Option<UsageScope>,
}

/// The `scope` object of a scoped limit — only the model display name is used.
#[derive(Debug, Deserialize)]
struct UsageScope {
    #[serde(default)]
    model: Option<ScopeModel>,
}

/// The model identity inside a scope (`{ "id": …, "display_name": "Fable" }`).
#[derive(Debug, Deserialize)]
struct ScopeModel {
    #[serde(default)]
    display_name: Option<String>,
}

/// One legacy utilization window. Tolerant of the field-name variants seen in the wild.
#[derive(Debug, Deserialize)]
struct UsageWindow {
    #[serde(alias = "percent", alias = "used_percentage", alias = "used_percent")]
    utilization: f64,
    /// `null` when the window is idle — mapped to `""` (no countdown).
    #[serde(default, alias = "reset_at")]
    resets_at: Option<String>,
}

/// Parse an `/api/oauth/usage` body into authoritative limits. Pure; `resets_at` kept verbatim.
/// Severity is classified against the configured thresholds (as for derived limits). Prefers the
/// canonical `limits[]` array; falls back to the legacy flat windows when the array is absent.
pub fn parse_oauth_usage(
    bytes: &[u8],
    account_id: &str,
    provider: Provider,
    warn_pct: f64,
    crit_pct: f64,
) -> AppResult<Vec<Limit>> {
    let usage: OauthUsage = serde_json::from_slice(bytes)
        .map_err(|e| AppError::Overlay(format!("malformed usage body: {e}")))?;

    let build = |kind, scope: Option<String>, pct: f64, resets_at: String| Limit {
        account_id: account_id.to_string(),
        provider,
        kind,
        scope,
        utilization_pct: pct,
        resets_at,
        severity: severity_for(pct, warn_pct, crit_pct),
        source: Provenance::Authoritative,
    };

    if !usage.limits.is_empty() {
        return Ok(usage
            .limits
            .into_iter()
            .filter_map(|entry| {
                let (kind, scope) = classify_limit(&entry.kind, entry.scope)?;
                Some(build(
                    kind,
                    scope,
                    entry.percent,
                    entry.resets_at.unwrap_or_default(),
                ))
            })
            .collect());
    }

    // Legacy fallback: the flat `five_hour`/`seven_day*` windows.
    let mut limits = Vec::new();
    let mut push = |window: Option<UsageWindow>, kind, scope: Option<&str>| {
        if let Some(w) = window {
            limits.push(build(
                kind,
                scope.map(str::to_string),
                w.utilization,
                w.resets_at.unwrap_or_default(),
            ));
        }
    };
    push(usage.five_hour, LimitKind::Session, None);
    push(usage.seven_day, LimitKind::WeeklyAll, None);
    push(usage.seven_day_opus, LimitKind::WeeklyScoped, Some("opus"));
    push(
        usage.seven_day_sonnet,
        LimitKind::WeeklyScoped,
        Some("sonnet"),
    );
    Ok(limits)
}

/// Map a `limits[]` kind string to a `LimitKind` + scope label. Unknown kinds ⇒ `None` (skipped).
fn classify_limit(kind: &str, scope: Option<UsageScope>) -> Option<(LimitKind, Option<String>)> {
    match kind {
        "session" => Some((LimitKind::Session, None)),
        "weekly_all" => Some((LimitKind::WeeklyAll, None)),
        "weekly_scoped" => {
            let model = scope.and_then(|s| s.model).and_then(|m| m.display_name);
            Some((LimitKind::WeeklyScoped, model))
        }
        _ => None,
    }
}

/// The outcome of one overlay attempt, driving [`next_backoff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffOutcome {
    /// A successful fetch — reset to the base interval.
    Ok,
    /// A 429 (no `Retry-After`) — grow the interval.
    Throttled,
}

/// Pure backoff: success resets to `base`; a 429 doubles the current interval, capped at `cap`.
pub fn next_backoff(
    current_secs: u64,
    outcome: BackoffOutcome,
    base_secs: u64,
    cap_secs: u64,
) -> u64 {
    match outcome {
        BackoffOutcome::Ok => base_secs,
        BackoffOutcome::Throttled => current_secs.max(base_secs).saturating_mul(2).min(cap_secs),
    }
}

/// The overlay endpoint seam. `HttpUsageEndpoint` is the only network touch; tests use a canned one.
#[async_trait]
pub trait UsageEndpoint: Send + Sync {
    /// GET the usage body for `access_token`. `Err(RateLimited)` on 429 (drives backoff).
    async fn fetch(&self, access_token: &str) -> AppResult<Vec<u8>>;
}

/// The real reqwest (rustls) endpoint. Bounded by a request timeout; opt-in gated by the caller.
#[derive(Debug)]
pub struct HttpUsageEndpoint {
    client: reqwest::Client,
}

impl HttpUsageEndpoint {
    /// Build the shared client (created once; reused across polls).
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(OVERLAY_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| AppError::Overlay(format!("cannot build HTTP client: {e}")))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl UsageEndpoint for HttpUsageEndpoint {
    async fn fetch(&self, access_token: &str) -> AppResult<Vec<u8>> {
        let response = self
            .client
            .get(OVERLAY_URL)
            .bearer_auth(access_token)
            .header("anthropic-beta", OAUTH_BETA)
            .send()
            .await
            .map_err(|e| AppError::Overlay(format!("request failed: {e}")))?;

        let status = response.status();
        if status.as_u16() == 429 {
            return Err(AppError::RateLimited);
        }
        if !status.is_success() {
            return Err(AppError::Overlay(format!("HTTP {}", status.as_u16())));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| AppError::Overlay(format!("read body failed: {e}")))?;
        Ok(bytes.to_vec())
    }
}

/// A canned endpoint for tests — no network. Test-only.
#[cfg(test)]
#[derive(Debug)]
pub enum Canned {
    /// Return this body verbatim.
    Body(Vec<u8>),
    /// Simulate a 429.
    RateLimited,
    /// Simulate a transport failure.
    Fail,
}

/// A [`UsageEndpoint`] returning a canned outcome (no network). Test-only.
#[cfg(test)]
#[derive(Debug)]
pub struct CannedEndpoint {
    /// The canned outcome to return from `fetch`.
    pub canned: Canned,
}

#[cfg(test)]
#[async_trait]
impl UsageEndpoint for CannedEndpoint {
    async fn fetch(&self, _access_token: &str) -> AppResult<Vec<u8>> {
        match &self.canned {
            Canned::Body(bytes) => Ok(bytes.clone()),
            Canned::RateLimited => Err(AppError::RateLimited),
            Canned::Fail => Err(AppError::Overlay("canned failure".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Severity;

    const BODY: &[u8] = br#"{
        "five_hour":        { "utilization": 19.0, "resets_at": "2026-07-04T12:00:00Z" },
        "seven_day":        { "utilization": 91.0, "resets_at": "2026-07-08T00:00:00Z" },
        "seven_day_opus":   { "utilization": 40.0, "resets_at": "2026-07-08T00:00:00Z" }
    }"#;

    #[test]
    fn parses_named_windows_as_authoritative() {
        let limits = parse_oauth_usage(BODY, "acct", Provider::Claude, 75.0, 90.0).expect("parses");
        assert_eq!(limits.len(), 3);

        let session = &limits[0];
        assert_eq!(session.kind, LimitKind::Session);
        assert_eq!(session.source, Provenance::Authoritative);
        assert!((session.utilization_pct - 19.0).abs() < 1e-9);
        assert_eq!(session.resets_at, "2026-07-04T12:00:00Z");
        assert_eq!(session.severity, Severity::Ok);

        assert_eq!(limits[1].kind, LimitKind::WeeklyAll);
        assert_eq!(limits[1].severity, Severity::Crit); // 91% ≥ crit
        assert_eq!(limits[2].kind, LimitKind::WeeklyScoped);
        assert_eq!(limits[2].scope.as_deref(), Some("opus"));
    }

    // The current endpoint shape: a canonical `limits[]` array (plus many ignored sibling fields),
    // with a per-model weekly-scoped entry keyed by the model's display name.
    const ARRAY_BODY: &[u8] = br#"{
        "five_hour": { "utilization": 29.0, "resets_at": "2026-07-04T18:20:00.44+00:00" },
        "seven_day_opus": null,
        "extra_usage": { "is_enabled": false },
        "limits": [
            { "kind": "session",       "percent": 29, "resets_at": "2026-07-04T18:20:00.44+00:00", "scope": null },
            { "kind": "weekly_all",    "percent": 78, "resets_at": "2026-07-10T03:00:00.44+00:00", "scope": null },
            { "kind": "weekly_scoped", "percent": 92, "resets_at": "2026-07-10T03:00:00.44+00:00",
              "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } },
            { "kind": "monthly_future", "percent": 5, "resets_at": "2026-08-01T00:00:00Z" }
        ]
    }"#;

    #[test]
    fn parses_canonical_limits_array_with_scoped_model() {
        // warn 75 / crit 90 ⇒ 29 ok, 78 warn, 92 crit (matches the /usage screen).
        let limits =
            parse_oauth_usage(ARRAY_BODY, "acct", Provider::Claude, 75.0, 90.0).expect("parses");
        // The unknown "monthly_future" kind is skipped; the three known kinds remain.
        assert_eq!(limits.len(), 3);

        assert_eq!(limits[0].kind, LimitKind::Session);
        assert_eq!(limits[0].severity, Severity::Ok);
        assert!((limits[0].utilization_pct - 29.0).abs() < 1e-9);
        assert_eq!(limits[0].source, Provenance::Authoritative);
        // resets_at is kept verbatim (fractional seconds + numeric offset).
        assert_eq!(limits[0].resets_at, "2026-07-04T18:20:00.44+00:00");

        assert_eq!(limits[1].kind, LimitKind::WeeklyAll);
        assert_eq!(limits[1].severity, Severity::Warn);

        assert_eq!(limits[2].kind, LimitKind::WeeklyScoped);
        assert_eq!(limits[2].scope.as_deref(), Some("Fable"));
        assert_eq!(limits[2].severity, Severity::Crit);
    }

    #[test]
    fn array_takes_precedence_over_legacy_flat_fields() {
        // Body carries BOTH the array and a flat five_hour; the array wins (no duplicate session).
        let sessions = parse_oauth_usage(ARRAY_BODY, "a", Provider::Claude, 75.0, 90.0)
            .expect("parses")
            .into_iter()
            .filter(|l| l.kind == LimitKind::Session)
            .count();
        assert_eq!(sessions, 1);
    }

    // Seen live 2026-07-08: an account with NO active 5h session reports `"resets_at": null` on the
    // session entry (and the flat five_hour). One null must not discard the whole body — the weekly
    // rows are exactly what matters when an account is pinned at its weekly limit.
    const IDLE_SESSION_BODY: &[u8] = br#"{
        "five_hour": { "utilization": 0.0, "resets_at": null },
        "seven_day_opus": null,
        "limits": [
            { "kind": "session",       "percent": 0,  "resets_at": null, "scope": null, "is_active": false },
            { "kind": "weekly_all",    "percent": 95, "resets_at": "2026-07-10T02:59:59.555258+00:00", "scope": null },
            { "kind": "weekly_scoped", "percent": 99, "resets_at": "2026-07-10T02:59:59.555681+00:00",
              "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } }
        ]
    }"#;

    #[test]
    fn tolerates_null_resets_at_on_idle_session() {
        let limits = parse_oauth_usage(IDLE_SESSION_BODY, "acct", Provider::Claude, 75.0, 90.0)
            .expect("null resets_at must not fail the whole body");
        assert_eq!(limits.len(), 3);
        assert_eq!(limits[0].kind, LimitKind::Session);
        assert_eq!(limits[0].resets_at, ""); // idle: no countdown, rendered without a "resets" tail
        assert_eq!(limits[1].kind, LimitKind::WeeklyAll);
        assert_eq!(limits[1].severity, Severity::Crit);
        assert_eq!(limits[2].scope.as_deref(), Some("Fable"));
        assert_eq!(limits[2].severity, Severity::Crit);
    }

    #[test]
    fn tolerates_percent_alias() {
        let body = br#"{ "five_hour": { "percent": 55.0, "resets_at": "2026-07-04T12:00:00Z" } }"#;
        let limits = parse_oauth_usage(body, "a", Provider::Claude, 75.0, 90.0).expect("parses");
        assert!((limits[0].utilization_pct - 55.0).abs() < 1e-9);
    }

    #[test]
    fn malformed_body_is_an_error() {
        assert!(parse_oauth_usage(b"nope", "a", Provider::Claude, 75.0, 90.0).is_err());
    }

    #[test]
    fn backoff_grows_on_throttle_and_resets_on_ok() {
        let base = 60;
        let cap = 600;
        let a = next_backoff(0, BackoffOutcome::Throttled, base, cap);
        assert_eq!(a, 120);
        let b = next_backoff(a, BackoffOutcome::Throttled, base, cap);
        assert_eq!(b, 240);
        let c = next_backoff(b, BackoffOutcome::Throttled, base, cap);
        assert_eq!(c, 480);
        let d = next_backoff(c, BackoffOutcome::Throttled, base, cap);
        assert_eq!(d, 600); // capped
        assert_eq!(next_backoff(d, BackoffOutcome::Throttled, base, cap), 600);
        assert_eq!(next_backoff(d, BackoffOutcome::Ok, base, cap), 60); // reset
    }

    #[tokio::test]
    async fn canned_endpoint_maps_rate_limit() {
        let endpoint = CannedEndpoint {
            canned: Canned::RateLimited,
        };
        assert!(matches!(
            endpoint.fetch("tok").await,
            Err(AppError::RateLimited)
        ));
    }
}
