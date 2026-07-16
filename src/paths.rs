//! Where tokenomics reads its config and writes its store — resolved independently of the cwd.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/paths.rs
//! Deps:    directories (XDG base dirs)
//! Tested:  inline `#[cfg(test)]` — the pure override picker + parent-dir selection.
//!
//! Key responsibilities:
//! - Resolve the config path (`$TOKENOMICS_CONFIG` → XDG config) and the store path
//!   (`$TOKENOMICS_DB` → XDG data), the same way from every working directory.
//!
//! Design constraints:
//! - No repo-/cwd-relative magic: a TUI must behave identically launched from any directory. The
//!   only way to point at a non-default file is an explicit env override (used for dev and tests).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Env var overriding the config file path (absolute, or relative to the cwd). Empty ⇒ ignored.
pub const CONFIG_ENV: &str = "TOKENOMICS_CONFIG";
/// Env var overriding the SQLite store path. Empty ⇒ ignored.
pub const STORE_ENV: &str = "TOKENOMICS_DB";

/// Which XDG base directory a lookup wants.
enum Base {
    Config,
    Data,
}

/// Resolve the config file path: `$TOKENOMICS_CONFIG` if set (non-empty), else the XDG config dir.
pub fn config_path() -> AppResult<PathBuf> {
    let xdg = xdg_dir(&Base::Config)?.join("tokenomics.toml");
    Ok(pick(std::env::var_os(CONFIG_ENV).as_deref(), &xdg))
}

/// The config file's mtime as epoch-ms, or `None` when the path can't be resolved or stat'd. Shared
/// by `tok doctor` (divergence check) and the collector's `--once` stamp so both read the same clock
/// as the hot-reload heartbeat stamp (spec 015 §B).
pub fn config_mtime_ms() -> Option<i64> {
    file_mtime_ms(&config_path().ok()?)
}

/// An arbitrary file's mtime as epoch-ms, or `None` when it can't be stat'd. Doctor uses it on the
/// path the collector *recorded* (which may differ from doctor's own resolution) and on the recorded
/// executable path — never re-resolving them itself (spec 015 §B/§B2).
pub fn file_mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(ms).ok()
}

/// The running executable's resolved path + mtime (epoch-ms), for the heartbeat's rebuild-staleness
/// stamp (spec 015 §B2). Captured once at collector start; doctor compares the recorded mtime to the
/// path's current mtime to flag a binary rebuilt after the collector started. `(None, None)` when
/// `current_exe()` or its stat fails — doctor stays silent on absent data.
pub fn current_exe_stamp() -> (Option<String>, Option<i64>) {
    match std::env::current_exe() {
        Ok(path) => {
            let mtime = file_mtime_ms(&path);
            (Some(path.display().to_string()), mtime)
        }
        Err(_) => (None, None),
    }
}

/// Resolve the store path: `$TOKENOMICS_DB` if set (non-empty), else the XDG data dir. Ensures the
/// parent directory exists so `Store::open` can create the file.
pub fn store_path() -> AppResult<PathBuf> {
    let xdg = xdg_dir(&Base::Data)?.join("tokenomics.db");
    let path = pick(std::env::var_os(STORE_ENV).as_deref(), &xdg);
    if let Some(parent) = parent_to_create(&path) {
        std::fs::create_dir_all(parent).map_err(|e| {
            AppError::StoreData(format!("cannot create data dir {}: {e}", parent.display()))
        })?;
    }
    Ok(path)
}

/// The platform config/data directory for this app, via `directories::ProjectDirs`:
/// - Linux: `~/.config/tokenomics`, `~/.local/share/tokenomics`
/// - macOS: `~/Library/Application Support/tokenomics` for both
///
/// Not "XDG" — that is the Linux answer only. Hardcoding it in docs sent macOS users to a path
/// that does not exist (spec 014); `tok init`/`validate`/`doctor` print the resolved path.
fn xdg_dir(base: &Base) -> AppResult<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "tokenomics").ok_or(AppError::NoConfigDir)?;
    Ok(match base {
        Base::Config => dirs.config_dir().to_path_buf(),
        Base::Data => dirs.data_dir().to_path_buf(),
    })
}

/// Pure override picker: a non-empty override wins; otherwise the default. The cwd plays no role.
fn pick(override_var: Option<&OsStr>, default: &Path) -> PathBuf {
    match override_var {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => default.to_path_buf(),
    }
}

/// The directory to create for `path`, or `None` when there is nothing meaningful to create
/// (a bare relative filename, whose parent is the already-existing cwd).
fn parent_to_create(path: &Path) -> Option<&Path> {
    path.parent().filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn non_empty_override_wins() {
        let default = Path::new("/xdg/tokenomics.toml");
        let over = OsString::from("/custom/my.toml");
        assert_eq!(
            pick(Some(over.as_os_str()), default),
            PathBuf::from("/custom/my.toml")
        );
    }

    #[test]
    fn empty_or_absent_override_falls_back_to_default() {
        let default = Path::new("/xdg/tokenomics.db");
        let empty = OsString::new();
        assert_eq!(
            pick(Some(empty.as_os_str()), default),
            default.to_path_buf()
        );
        assert_eq!(pick(None, default), default.to_path_buf());
    }

    #[test]
    fn parent_to_create_skips_bare_relative_names() {
        assert_eq!(
            parent_to_create(Path::new("/a/b/c.db")),
            Some(Path::new("/a/b"))
        );
        assert_eq!(parent_to_create(Path::new("tokenomics.db")), None);
        assert_eq!(
            parent_to_create(Path::new("./sub/t.db")),
            Some(Path::new("./sub"))
        );
    }
}
