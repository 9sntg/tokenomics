//! Typed crate error and result alias.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/error.rs
//! Deps:    thiserror, rusqlite (store error `#[from]`)
//! Tested:  exercised via config.rs / providers / store tests
//!
//! Key responsibilities:
//! - `AppError`: the single typed error the app propagates internally.
//! - `AppResult<T>`: the internal result alias (`anyhow` is reserved for `main.rs` edges).
//!
//! Design constraints:
//! - Never embed a secret (OAuth token) in an error message.

use std::path::PathBuf;

use thiserror::Error;

/// The crate-wide typed error. New variants are added by the wave that first needs them.
#[derive(Debug, Error)]
pub enum AppError {
    /// The config file could not be parsed as valid `tokenomics.toml`.
    #[error("config parse error: {0}")]
    ConfigParse(String),
    /// The config file exists but could not be read.
    #[error("cannot read config at {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// No config directory could be resolved on this platform.
    #[error("could not determine a config directory")]
    NoConfigDir,
    /// An external command failed to spawn or exited non-zero. `message` never carries a secret
    /// (ccusage output has none; token-bearing calls redact before constructing this).
    #[error("subprocess `{program}` failed: {message}")]
    Subprocess {
        /// The program that was invoked (argv[0]).
        program: String,
        /// A short, secret-free failure summary (exit code + stderr tail, or spawn error).
        message: String,
    },
    /// An external command exceeded its per-call timeout.
    #[error("subprocess `{program}` timed out after {seconds}s")]
    Timeout {
        /// The program that timed out.
        program: String,
        /// The elapsed budget in seconds.
        seconds: u64,
    },
    /// ccusage output could not be parsed as the expected JSON shape.
    #[error("could not parse ccusage output: {0}")]
    CcusageParse(String),
    /// The Codex sessions scan's blocking task could not be joined (runtime shutdown, or a
    /// bug-level panic in the walker). Carries no path contents, only the join error.
    #[error("codex sessions scan failed: {0}")]
    SessionsScan(String),
    /// The SQLite store failed (open, migrate, or a query).
    #[error("store error: {0}")]
    Store(#[from] rusqlite::Error),
    /// A stored row held a value that could not be decoded back into a domain type.
    #[error("store data error: {0}")]
    StoreData(String),
    /// The terminal could not be set up, drawn, or restored (not a TTY, or an I/O failure).
    #[error("terminal error: {0}")]
    Terminal(String),
    /// A credentials problem (missing, wrong file mode, or unparseable). Never carries the token.
    #[error("credentials error: {0}")]
    Credentials(String),
    /// The opt-in overlay failed (parse, transport, or a stale token). Never carries the token.
    #[error("overlay error: {0}")]
    Overlay(String),
    /// The overlay endpoint returned HTTP 429 (throttled) — the caller backs off.
    #[error("overlay rate-limited")]
    RateLimited,
}

/// Internal result alias. `main.rs` may lift these into `anyhow` at the edge.
pub type AppResult<T> = Result<T, AppError>;
