//! Codex limits overlay: `codex app-server` JSON-RPC `account/rateLimits/read` → `Limit`s.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/providers/codex/rate_limits.rs
//! Deps:    serde_json, jiff, tokio (process); format::severity_for
//! Tested:  inline `#[cfg(test)]` — pure response parse + a canned-transcript client seam
//!
//! Key responsibilities:
//! - `parse_rate_limits_response`: primary → Session, secondary → WeeklyAll; `usedPercent` →
//!   `utilization_pct`; epoch `resetsAt` → RFC 3339 `resets_at`; provenance `Authoritative`
//!   (spec 013 §C).
//! - The subprocess client behind an injectable seam: argv-only, `CODEX_HOME` pinned, stdin held
//!   open, one hard timeout, `kill_on_drop` — no secret read, logged, or stored.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use jiff::Timestamp;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::domain::{Limit, LimitKind, Provenance, Provider};
use crate::error::{AppError, AppResult};
use crate::format::severity_for;

/// The program invoked for the limits fetch (resolved via `PATH`); `argv[1]` is its subcommand.
const CODEX_PROGRAM: &str = "codex";
/// The `codex` subcommand that speaks JSON-RPC over stdio.
const APP_SERVER_ARG: &str = "app-server";
/// The invocation label used in error `program` fields (never a shell string).
const CODEX_INVOCATION: &str = "codex app-server";
/// The env var that pins the app-server to this account's config dir — the only attribution handle.
const CODEX_HOME_ENV: &str = "CODEX_HOME";
/// The JSON-RPC `id` of our `account/rateLimits/read` request — the response we read stdout for.
const RATE_LIMITS_ID: i64 = 1;
/// The `initialize` handshake the app-server requires before it answers any request.
const INITIALIZE_REQUEST: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"clientInfo":{"name":"tokenomics","title":"tok","version":"0.1"}}}"#;
/// The rate-limits request; its `id` matches [`RATE_LIMITS_ID`].
const RATE_LIMITS_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":1,"method":"account/rateLimits/read","params":{}}"#;

/// A JSON-RPC response body — only the fields we consume (`result`/`error`; unknown ones ignored).
#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Option<RpcResult>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

/// The `result` object of the rate-limits response.
#[derive(Debug, Deserialize)]
struct RpcResult {
    #[serde(default, rename = "rateLimits")]
    rate_limits: Option<RateLimits>,
}

/// The `rateLimits` object: `primary` (5h session) and `secondary` (weekly). Each may be absent.
#[derive(Debug, Deserialize)]
struct RateLimits {
    #[serde(default)]
    primary: Option<RateLimitWindow>,
    #[serde(default)]
    secondary: Option<RateLimitWindow>,
}

/// One rate-limit window: server-authoritative percent + an epoch-seconds reset.
#[derive(Debug, Deserialize)]
struct RateLimitWindow {
    /// May be fractional — accepted as `f64`.
    #[serde(rename = "usedPercent")]
    used_percent: f64,
    /// Epoch seconds. Absent/null ⇒ no countdown (rendered as an empty `resets_at`).
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<i64>,
}

/// The `id` field only — used to pick the response line out of interleaved notifications.
#[derive(Debug, Deserialize)]
struct IdOnly {
    #[serde(default)]
    id: Option<i64>,
}

/// The JSON-RPC response `id` of `line`, if it parses and carries one. Notification lines (no `id`)
/// and non-JSON lines yield `None` and are skipped by the callers.
fn line_response_id(line: &str) -> Option<i64> {
    serde_json::from_str::<IdOnly>(line)
        .ok()
        .and_then(|parsed| parsed.id)
}

/// Parse the app-server output into authoritative Codex limits. Pure. `lines` is the stdout of the
/// exchange (or just the matched line); the `account/rateLimits/read` response is located by its
/// `id`, skipping unrelated notification lines. `primary` → `Session`, `secondary` → `WeeklyAll`
/// (Codex has no per-model scoped weeklies). `usedPercent` → `utilization_pct`; `resetsAt` (epoch
/// seconds) → an RFC 3339 UTC `resets_at` normalized once here and rendered verbatim thereafter;
/// severity via [`severity_for`]; provenance [`Provenance::Authoritative`]. A missing response, an
/// `error` reply, or a malformed body ⇒ `Err` (the caller degrades, never invents).
pub fn parse_rate_limits_response(
    lines: &[&str],
    account_id: &str,
    warn_pct: f64,
    crit_pct: f64,
) -> AppResult<Vec<Limit>> {
    let line = lines
        .iter()
        .copied()
        .find(|line| line_response_id(line) == Some(RATE_LIMITS_ID))
        .ok_or_else(|| {
            AppError::Overlay(
                "no account/rateLimits/read response in app-server output".to_string(),
            )
        })?;

    // The line was matched by id already; a body that still fails to parse is a genuine malformation.
    // The error message stays generic — never echo a raw stdout line (auth could surface in one).
    let response: RpcResponse = serde_json::from_str(line)
        .map_err(|_| AppError::Overlay("malformed account/rateLimits/read response".to_string()))?;

    if response.error.is_some() {
        return Err(AppError::Overlay(
            "app-server returned an error for account/rateLimits/read".to_string(),
        ));
    }

    let rate_limits = response
        .result
        .and_then(|result| result.rate_limits)
        .ok_or_else(|| {
            AppError::Overlay("account/rateLimits/read response carried no rateLimits".to_string())
        })?;

    let mut limits = Vec::new();
    if let Some(primary) = rate_limits.primary {
        limits.push(window_to_limit(
            &primary,
            account_id,
            LimitKind::Session,
            warn_pct,
            crit_pct,
        )?);
    }
    if let Some(secondary) = rate_limits.secondary {
        limits.push(window_to_limit(
            &secondary,
            account_id,
            LimitKind::WeeklyAll,
            warn_pct,
            crit_pct,
        )?);
    }
    Ok(limits)
}

/// Map one window to a `Limit`. `resetsAt` (epoch seconds) becomes an RFC 3339 UTC string; absent ⇒
/// empty (no countdown). An out-of-range epoch ⇒ `Err`.
fn window_to_limit(
    window: &RateLimitWindow,
    account_id: &str,
    kind: LimitKind,
    warn_pct: f64,
    crit_pct: f64,
) -> AppResult<Limit> {
    let resets_at = match window.resets_at {
        Some(epoch_secs) => Timestamp::from_second(epoch_secs)
            .map_err(|_| {
                AppError::Overlay(
                    "account/rateLimits/read carried an out-of-range resetsAt".to_string(),
                )
            })?
            .to_string(),
        None => String::new(),
    };
    Ok(Limit {
        account_id: account_id.to_string(),
        provider: Provider::Codex,
        kind,
        scope: None,
        utilization_pct: window.used_percent,
        resets_at,
        severity: severity_for(window.used_percent, warn_pct, crit_pct),
        source: Provenance::Authoritative,
    })
}

/// The rate-limits source seam. `AppServerClient` drives the real subprocess; tests use a canned one.
#[async_trait]
pub trait RateLimitsSource: Send + Sync {
    /// Fetch the raw `account/rateLimits/read` response line for the account at `config_dir`
    /// (its `CODEX_HOME`). Bounded internally by one hard timeout; no secret is read or returned.
    async fn fetch(&self, config_dir: &Path) -> AppResult<String>;
}

/// The real client: an interactive `codex app-server` JSON-RPC exchange over stdio. Argv-only,
/// `CODEX_HOME` pinned, stdin held open, whole exchange under one hard timeout, `kill_on_drop`.
#[derive(Debug, Clone, Copy)]
pub struct AppServerClient {
    /// The hard ceiling on the whole spawn/write/read exchange (supplied by the caller).
    timeout: Duration,
}

impl AppServerClient {
    /// Build a client bounding every exchange by `timeout`.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// The spawn → write → read exchange, without the timeout wrapper (applied by [`fetch`]).
    async fn exchange(&self, config_dir: &Path) -> AppResult<String> {
        let mut child = Command::new(CODEX_PROGRAM)
            .arg(APP_SERVER_ARG)
            .env(CODEX_HOME_ENV, config_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| AppError::Subprocess {
                program: CODEX_INVOCATION.to_string(),
                message: err.to_string(),
            })?;

        let mut stdin = child.stdin.take().ok_or_else(|| AppError::Subprocess {
            program: CODEX_INVOCATION.to_string(),
            message: "app-server stdin was not captured".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| AppError::Subprocess {
            program: CODEX_INVOCATION.to_string(),
            message: "app-server stdout was not captured".to_string(),
        })?;

        // Write both requests, then HOLD stdin open (the `stdin` handle lives until this function
        // returns) — the child emits nothing if stdin closes immediately.
        for request in [INITIALIZE_REQUEST, RATE_LIMITS_REQUEST] {
            stdin.write_all(request.as_bytes()).await.map_err(io_err)?;
            stdin.write_all(b"\n").await.map_err(io_err)?;
        }
        stdin.flush().await.map_err(io_err)?;

        let mut reader = BufReader::new(stdout).lines();
        while let Some(line) = reader.next_line().await.map_err(io_err)? {
            if line_response_id(&line) == Some(RATE_LIMITS_ID) {
                return Ok(line);
            }
        }
        Err(AppError::Overlay(
            "app-server closed before answering account/rateLimits/read".to_string(),
        ))
        // `stdin` and `child` drop here; `kill_on_drop` reaps the child.
    }
}

/// Map a stdio I/O error to a secret-free subprocess error (I/O errors carry no stdout content).
// `map_err` hands the `io::Error` by value; formatting only borrows it — hence the allow.
#[allow(clippy::needless_pass_by_value)]
fn io_err(err: std::io::Error) -> AppError {
    AppError::Subprocess {
        program: CODEX_INVOCATION.to_string(),
        message: format!("app-server stdio failed: {err}"),
    }
}

#[async_trait]
impl RateLimitsSource for AppServerClient {
    async fn fetch(&self, config_dir: &Path) -> AppResult<String> {
        match tokio::time::timeout(self.timeout, self.exchange(config_dir)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(AppError::Timeout {
                program: CODEX_INVOCATION.to_string(),
                seconds: self.timeout.as_secs(),
            }),
        }
    }
}

/// A [`RateLimitsSource`] returning a canned response line (no process spawn). Test-only.
#[cfg(test)]
#[derive(Debug)]
pub struct CannedSource {
    /// The `account/rateLimits/read` response line to hand back verbatim.
    pub response: String,
}

#[cfg(test)]
#[async_trait]
impl RateLimitsSource for CannedSource {
    async fn fetch(&self, _config_dir: &Path) -> AppResult<String> {
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Severity;

    // The live shape verified on this machine (codex-cli 0.144.1): id 1 result with primary +
    // secondary windows and many sibling fields serde ignores.
    const REAL_RESPONSE: &str = r#"{"id":1,"result":{"rateLimits":{"limitId":"codex","limitName":null,"primary":{"usedPercent":1,"windowDurationMins":300,"resetsAt":1783733780},"secondary":{"usedPercent":0,"windowDurationMins":10080,"resetsAt":1784320580},"credits":{"hasCredits":false,"unlimited":false,"balance":null},"individualLimit":null,"planType":"team","rateLimitReachedType":null},"rateLimitsByLimitId":{},"rateLimitResetCredits":{}}}"#;

    #[test]
    fn maps_primary_and_secondary_to_session_and_weekly() {
        let limits =
            parse_rate_limits_response(&[REAL_RESPONSE], "codex-acct", 75.0, 90.0).expect("parses");
        assert_eq!(limits.len(), 2);

        let session = &limits[0];
        assert_eq!(session.account_id, "codex-acct");
        assert_eq!(session.provider, Provider::Codex);
        assert_eq!(session.kind, LimitKind::Session);
        assert_eq!(session.scope, None);
        assert_eq!(session.source, Provenance::Authoritative);
        assert_eq!(session.severity, Severity::Ok);
        assert!((session.utilization_pct - 1.0).abs() < 1e-9);
        // resets_at is a valid RFC 3339 UTC string round-tripping to the source epoch.
        let parsed: Timestamp = session.resets_at.parse().expect("rfc3339");
        assert_eq!(parsed.as_second(), 1_783_733_780);
        assert!(session.resets_at.ends_with('Z'));

        let weekly = &limits[1];
        assert_eq!(weekly.kind, LimitKind::WeeklyAll);
        assert_eq!(weekly.scope, None);
        assert_eq!(weekly.source, Provenance::Authoritative);
    }

    #[test]
    fn epoch_seconds_become_rfc3339_utc() {
        let line = r#"{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":0,"resetsAt":0}}}}"#;
        let limits = parse_rate_limits_response(&[line], "a", 75.0, 90.0).expect("parses");
        assert_eq!(limits[0].resets_at, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn accepts_fractional_percent() {
        let line = r#"{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":12.5,"resetsAt":1783733780}}}}"#;
        let limits = parse_rate_limits_response(&[line], "a", 75.0, 90.0).expect("parses");
        assert_eq!(limits.len(), 1);
        assert!((limits[0].utilization_pct - 12.5).abs() < 1e-9);
    }

    #[test]
    fn null_secondary_yields_only_session() {
        let line = r#"{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":50,"resetsAt":1783733780},"secondary":null}}}"#;
        let limits = parse_rate_limits_response(&[line], "a", 75.0, 90.0).expect("parses");
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].kind, LimitKind::Session);
    }

    #[test]
    fn severity_classified_at_warn_and_crit_boundaries() {
        let line = r#"{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":90,"resetsAt":1783733780},"secondary":{"usedPercent":75,"resetsAt":1784320580}}}}"#;
        let limits = parse_rate_limits_response(&[line], "a", 75.0, 90.0).expect("parses");
        assert_eq!(limits[0].severity, Severity::Crit); // 90 ≥ crit
        assert_eq!(limits[1].severity, Severity::Warn); // 75 ≥ warn, < crit
    }

    #[test]
    fn absent_resets_at_renders_empty() {
        let line = r#"{"id":1,"result":{"rateLimits":{"primary":{"usedPercent":50}}}}"#;
        let limits = parse_rate_limits_response(&[line], "a", 75.0, 90.0).expect("parses");
        assert_eq!(limits[0].resets_at, "");
    }

    #[test]
    fn skips_notification_and_other_id_lines() {
        let notification = r#"{"jsonrpc":"2.0","method":"remoteControl/status/changed","params":{"status":"idle"}}"#;
        let initialize = r#"{"id":0,"result":{"userAgent":"codex","protocolVersion":1}}"#;
        let limits =
            parse_rate_limits_response(&[notification, initialize, REAL_RESPONSE], "a", 75.0, 90.0)
                .expect("parses past interleaved lines");
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].kind, LimitKind::Session);
    }

    #[test]
    fn error_reply_is_an_error() {
        let line = r#"{"id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        assert!(parse_rate_limits_response(&[line], "a", 75.0, 90.0).is_err());
    }

    #[test]
    fn missing_rate_limits_is_an_error() {
        let line = r#"{"id":1,"result":{"somethingElse":true}}"#;
        assert!(parse_rate_limits_response(&[line], "a", 75.0, 90.0).is_err());
    }

    #[test]
    fn absent_response_line_is_an_error() {
        let notification = r#"{"jsonrpc":"2.0","method":"x","params":{}}"#;
        assert!(parse_rate_limits_response(&[notification], "a", 75.0, 90.0).is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(
            parse_rate_limits_response(&["not json", "{also broken"], "a", 75.0, 90.0).is_err()
        );
    }

    #[tokio::test]
    async fn canned_source_composes_with_parse() {
        let source = CannedSource {
            response: REAL_RESPONSE.to_string(),
        };
        let line = source
            .fetch(Path::new("/does/not/matter"))
            .await
            .expect("canned");
        let limits =
            parse_rate_limits_response(&[line.as_str()], "codex-acct", 75.0, 90.0).expect("parses");
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].kind, LimitKind::Session);
        assert_eq!(limits[1].kind, LimitKind::WeeklyAll);
    }
}
