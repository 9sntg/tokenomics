//! Read the account's Claude OAuth token for the opt-in overlay — passively, never mutating it.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/providers/claude/creds.rs
//! Deps:    serde_json, jiff; std::fs (thin read + mode check)
//! Tested:  inline `#[cfg(test)]` — parse, warmth, redaction (no real credentials touched)
//!
//! Key responsibilities:
//! - `parse_credentials`: bytes → `TokenInfo { access_token (secret), expires_at_ms }` (pure).
//! - `read_token`: read `<config_dir>/.credentials.json`, require mode `0600` (Unix), parse.
//!
//! Design constraints:
//! - The access token is a SECRET: `TokenInfo`'s `Debug` redacts it; it never enters a log, error,
//!   or the store. Error messages never include the file bytes (which contain the token).
//! - Read-only: we reuse Claude Code's token passively; we never write the credentials file here.
//! - **macOS has no credentials file, and we deliberately do not read the Keychain** (spec 014).
//!   Claude Code stores the token there (service `Claude Code-credentials`); its payload has the
//!   same shape this module parses, so a port would be small. It is declined on purpose: the
//!   token's only consumer is the opt-in `/api/oauth/usage` overlay, which the README documents
//!   as NOT PERMITTED under Anthropic's 2026 Consumer-Terms clarification. Reading it would be
//!   code whose sole purpose is easing a not-permitted action.
//! - Only the ABSENT-file message is platform-aware (`no_source_note`) — reading stays identical
//!   everywhere, so the overlay's own tests (which write a real file) run unchanged on macOS.

use std::path::Path;

use serde::Deserialize;

use crate::error::{AppError, AppResult};

/// One account's OAuth access token + its expiry. The token is a secret (redacted in `Debug`).
#[derive(Clone)]
pub struct TokenInfo {
    access_token: String,
    /// Expiry as epoch-milliseconds (Claude stores a 13-digit ms value).
    pub expires_at_ms: i64,
}

impl TokenInfo {
    /// The bearer token — used ONLY to build the `Authorization` header. Never log this.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Whether the token is still valid at `now_ms` (warm ⇒ safe to use passively).
    pub fn is_warm(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }
}

impl std::fmt::Debug for TokenInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenInfo")
            .field("access_token", &"<redacted>")
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude: Option<ClaudeOauth>,
}

#[derive(Deserialize)]
struct ClaudeOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
}

/// Parse `.credentials.json` bytes into a `TokenInfo`. Pure. On any failure returns a generic
/// message — never the raw bytes, which contain the token.
pub fn parse_credentials(bytes: &[u8]) -> AppResult<TokenInfo> {
    let file: CredentialsFile = serde_json::from_slice(bytes)
        .map_err(|_| AppError::Credentials("malformed .credentials.json".to_string()))?;
    let oauth = file.claude.ok_or_else(|| {
        AppError::Credentials("no claudeAiOauth token in credentials".to_string())
    })?;
    Ok(TokenInfo {
        access_token: oauth.access_token,
        expires_at_ms: oauth.expires_at,
    })
}

/// Read and parse `<config_dir>/.credentials.json`, requiring owner-only file mode on Unix.
///
/// Cross-platform on purpose: if a file is there, it is read the same way everywhere. Only the
/// ABSENT case differs — on macOS there is no such file by design, and `cannot stat …` would read
/// like a bug to fix (spec 014). See `no_source_note`.
pub fn read_token(config_dir: &Path) -> AppResult<TokenInfo> {
    let path = config_dir.join(".credentials.json");
    if !path.exists() {
        return Err(AppError::Credentials(no_source_note().to_string()));
    }
    require_owner_only(&path)?;
    let bytes = std::fs::read(&path).map_err(|e| {
        AppError::Credentials(format!("cannot read {}: {}", path.display(), e.kind()))
    })?;
    parse_credentials(&bytes)
}

/// Why there is no credentials file — the one place that knows the platform.
///
/// macOS: Claude Code keeps the token in the Keychain (service `Claude Code-credentials`). We
/// deliberately do NOT read it — the token's only consumer is the opt-in `/api/oauth/usage`
/// overlay, which the README documents as NOT PERMITTED under Anthropic's 2026 Consumer-Terms
/// clarification. So this states the situation and stops: it names neither a path that cannot
/// exist here nor a recipe for extracting the token.
#[cfg(target_os = "macos")]
pub const fn no_source_note() -> &'static str {
    "n/a on macOS — Claude Code keeps the token in the Keychain; the overlay is unsupported here \
     and the local plane needs none"
}

/// Elsewhere the file is simply absent: the account never logged in, or the overlay was never
/// wanted. Either way the local plane is unaffected.
#[cfg(not(target_os = "macos"))]
pub const fn no_source_note() -> &'static str {
    "absent — the overlay needs <config_dir>/.credentials.json; the local plane does not"
}

/// Refuse a credentials file that is group/world-accessible (Unix only). A leaked token is a P0.
#[cfg(unix)]
fn require_owner_only(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|e| {
        AppError::Credentials(format!("cannot stat {}: {}", path.display(), e.kind()))
    })?;
    let mode = meta.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(AppError::Credentials(format!(
            "{} is group/world-accessible (mode {:o}); refusing — chmod 600",
            path.display(),
            mode & 0o777
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_owner_only(_path: &Path) -> AppResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDS: &[u8] = br#"{
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat-SECRET-VALUE",
            "refreshToken": "sk-ant-ort-SECRET",
            "expiresAt": 1782000000000,
            "scopes": ["a","b","c","d","e"],
            "subscriptionType": "max",
            "rateLimitTier": "default"
        }
    }"#;

    #[test]
    fn parses_token_and_expiry() {
        let token = parse_credentials(CREDS).expect("parses");
        assert_eq!(token.access_token(), "sk-ant-oat-SECRET-VALUE");
        assert_eq!(token.expires_at_ms, 1_782_000_000_000);
    }

    #[test]
    fn warmth_compares_expiry_to_now() {
        let token = parse_credentials(CREDS).expect("parses");
        assert!(token.is_warm(1_781_000_000_000)); // before expiry
        assert!(!token.is_warm(1_783_000_000_000)); // after expiry
    }

    #[test]
    fn debug_never_leaks_the_token() {
        let token = parse_credentials(CREDS).expect("parses");
        let dumped = format!("{token:?}");
        assert!(dumped.contains("<redacted>"));
        assert!(!dumped.contains("SECRET"));
    }

    #[test]
    fn missing_oauth_block_is_an_error() {
        assert!(parse_credentials(br#"{"other":1}"#).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_read_token_names_the_keychain_not_a_phantom_file() {
        // Spec 014. The old path produced `cannot stat ~/.claude/.credentials.json: entity not
        // found` — a file macOS was never going to have, which reads as a bug to fix. The "fix"
        // would be reading the Keychain to feed a NOT-PERMITTED overlay, so the message must
        // explain instead of alarm.
        let err = read_token(Path::new("/Users/user/.claude")).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Keychain"),
            "must name where the token actually lives: {msg}"
        );
        assert!(
            !msg.contains("cannot stat") && !msg.contains(".credentials.json"),
            "must not point at a file that cannot exist here: {msg}"
        );
        assert!(
            !msg.contains("security find-generic-password"),
            "must not hand out a recipe for extracting the token"
        );
    }

    #[test]
    fn malformed_json_error_has_no_bytes() {
        let err = parse_credentials(b"sk-ant-oat-LEAK not json").unwrap_err();
        assert!(!format!("{err}").contains("LEAK"));
    }
}
