//! Pure core: Codex rollout-JSONL `token_count` events → a normalized `UsageSnapshot`.
//!
//! Project: Tokenomics — monitor LLM subscription accounts (usage, limits, time-left) in a TUI
//! Module:  src/providers/codex/sessions.rs
//! Deps:    serde_json, jiff (no I/O — bytes in, domain out)
//! Tested:  inline `#[cfg(test)]` on fixtures/codex_rollout.jsonl (real-shape)
//!
//! Key responsibilities:
//! - `parse_rollout_events`: extract timestamped `last_token_usage` deltas from one rollout file,
//!   skipping malformed/foreign lines defensively (spec 013 §B).
//! - `reduce_codex_snapshot`: sum in-window deltas into `UsageSnapshot` buckets
//!   (`input = input − cached`, `cache_read = cached`, `cache_creation = 0`; `cost_notional`
//!   and `window` stay `None` — no honest basis).
//!
//! Design constraints:
//! - A malformed *line* never fails the file — the caller gets whatever parsed.
//! - Time is injected (`now`) so reduction stays pure and deterministic.

use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;

use crate::domain::{Provider, UsageSnapshot};

/// The reduction lookback: sum only `last_token_usage` deltas from the trailing 5 hours.
const LOOKBACK: Duration = Duration::from_hours(5);

/// One `token_count` event's timestamp plus its per-turn (`last_token_usage`) delta buckets,
/// already isolated from the rollout envelope and ready to sum in [`reduce_codex_snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCountEvent {
    /// When this event was recorded.
    pub timestamp: Timestamp,
    /// Non-cache input tokens for this turn.
    pub input_tokens: u64,
    /// Cached (prompt-cache-read) input tokens for this turn.
    pub cached_input_tokens: u64,
    /// Output tokens for this turn (reasoning tokens are already included).
    pub output_tokens: u64,
    /// All buckets summed for this turn (the rollout's own per-turn `total_tokens`).
    pub total_tokens: u64,
}

/// One rollout-JSONL line's envelope — only the fields needed to find `token_count` events.
/// Foreign line types (`session_meta`, `response_item`, `turn_context`, `world_state`, …) either
/// fail this shape or fail the `kind` check below; both cases skip the line.
#[derive(Debug, Deserialize)]
struct RolloutLine {
    timestamp: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Option<Payload>,
}

/// The `payload` object of an `event_msg` line.
#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    info: Option<TokenCountInfo>,
}

/// `payload.info` of a `token_count` event (`null` in practice before the first reply — skipped).
#[derive(Debug, Deserialize)]
struct TokenCountInfo {
    last_token_usage: TokenUsage,
}

/// The `last_token_usage` buckets this crate maps into a [`UsageSnapshot`].
#[derive(Debug, Default, Deserialize)]
struct TokenUsage {
    #[serde(default, rename = "input_tokens")]
    input: u64,
    #[serde(default, rename = "cached_input_tokens")]
    cached_input: u64,
    #[serde(default, rename = "output_tokens")]
    output: u64,
    #[serde(default, rename = "total_tokens")]
    total: u64,
}

/// Extract `token_count` events from one rollout file's bytes (newline-delimited JSON). Any line
/// that isn't valid JSON, isn't an `event_msg`/`token_count`, or carries `info: null` is skipped —
/// a malformed line never fails the file (spec 013 §B).
pub fn parse_rollout_events(bytes: &[u8]) -> Vec<TokenCountEvent> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(parse_line)
        .collect()
}

/// Parse one line into a `TokenCountEvent`, or `None` if it isn't one.
fn parse_line(line: &[u8]) -> Option<TokenCountEvent> {
    let raw: RolloutLine = serde_json::from_slice(line).ok()?;
    if raw.kind != "event_msg" {
        return None;
    }
    let payload = raw.payload?;
    if payload.kind != "token_count" {
        return None;
    }
    let usage = payload.info?.last_token_usage;
    let timestamp: Timestamp = raw.timestamp.parse().ok()?;
    Some(TokenCountEvent {
        timestamp,
        input_tokens: usage.input,
        cached_input_tokens: usage.cached_input,
        output_tokens: usage.output,
        total_tokens: usage.total,
    })
}

/// Reduce parsed events to a normalized snapshot for one account: sum the `last_token_usage`
/// deltas of events timestamped within the trailing 5h of `now`. Bucket mapping: `input =
/// input_tokens − cached_input_tokens` (floored at 0 via saturating sub), `cache_read =
/// cached_input_tokens`, `output = output_tokens`, `cache_creation = 0`. No in-window events ⇒
/// `None` (idle — the `ProviderAdapter` contract). `cost_notional`/`window` stay `None` (no
/// honest cost basis; Codex exposes no local block).
pub fn reduce_codex_snapshot(
    events: &[TokenCountEvent],
    account_id: &str,
    now: Timestamp,
) -> Option<UsageSnapshot> {
    let cutoff = now - LOOKBACK;
    let mut snapshot = UsageSnapshot {
        account_id: account_id.to_string(),
        provider: Provider::Codex,
        collected_at: now,
        input: 0,
        output: 0,
        cache_read: 0,
        cache_creation: 0,
        total_tokens: 0,
        cost_notional: None,
        window: None,
    };
    let mut any = false;
    for event in events.iter().filter(|e| e.timestamp >= cutoff) {
        any = true;
        snapshot.input = snapshot
            .input
            .saturating_add(event.input_tokens.saturating_sub(event.cached_input_tokens));
        snapshot.cache_read = snapshot
            .cache_read
            .saturating_add(event.cached_input_tokens);
        snapshot.output = snapshot.output.saturating_add(event.output_tokens);
        snapshot.total_tokens = snapshot.total_tokens.saturating_add(event.total_tokens);
    }
    any.then_some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLLOUT: &[u8] = include_bytes!("../../../fixtures/codex_rollout.jsonl");

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("valid timestamp")
    }

    #[test]
    fn extracts_only_token_count_events_from_real_shape_fixture() {
        let events = parse_rollout_events(ROLLOUT);
        assert_eq!(events.len(), 2, "session_meta/response_item/turn_context/world_state/info:null/malformed lines are all skipped");
        assert_eq!(events[0].timestamp, ts("2026-07-10T15:05:00Z"));
        assert_eq!(events[0].input_tokens, 1000);
        assert_eq!(events[0].cached_input_tokens, 200);
        assert_eq!(events[0].output_tokens, 50);
        assert_eq!(events[0].total_tokens, 1050);

        assert_eq!(events[1].timestamp, ts("2026-07-10T20:27:05.632Z"));
        assert_eq!(events[1].input_tokens, 12_991);
        assert_eq!(events[1].cached_input_tokens, 12_945);
        assert_eq!(events[1].output_tokens, 100);
        assert_eq!(events[1].total_tokens, 13_091);
    }

    #[test]
    fn empty_bytes_parse_to_no_events() {
        assert!(parse_rollout_events(b"").is_empty());
    }

    #[test]
    fn entirely_malformed_bytes_parse_to_no_events_not_an_error() {
        assert!(parse_rollout_events(b"not json at all\nneither is this").is_empty());
    }

    fn event(timestamp: &str, input: u64, cached: u64, output: u64, total: u64) -> TokenCountEvent {
        TokenCountEvent {
            timestamp: ts(timestamp),
            input_tokens: input,
            cached_input_tokens: cached,
            output_tokens: output,
            total_tokens: total,
        }
    }

    #[test]
    fn reduce_sums_only_in_window_deltas_with_the_documented_bucket_mapping() {
        let now = ts("2026-07-10T20:30:00Z");
        // Cutoff is now - 5h = 2026-07-10T15:30:00Z.
        let events = vec![
            event("2026-07-10T15:29:59Z", 500, 100, 20, 550), // just before cutoff: excluded
            event("2026-07-10T15:30:00Z", 1000, 200, 50, 1050), // exactly at cutoff: included
            event("2026-07-10T18:00:00Z", 2000, 500, 100, 2500), // well within window
        ];

        let snap = reduce_codex_snapshot(&events, "acct", now).expect("in-window events present");
        assert_eq!(snap.account_id, "acct");
        assert_eq!(snap.provider, Provider::Codex);
        assert_eq!(snap.collected_at, now);
        assert_eq!(snap.input, 2_300); // (1000-200) + (2000-500)
        assert_eq!(snap.cache_read, 700); // 200 + 500
        assert_eq!(snap.cache_creation, 0);
        assert_eq!(snap.output, 150); // 50 + 100
        assert_eq!(snap.total_tokens, 3_550); // 1050 + 2500
        assert_eq!(snap.cost_notional, None);
        assert!(snap.window.is_none());
    }

    #[test]
    fn no_in_window_events_reduce_to_none() {
        let now = ts("2026-07-10T20:30:00Z");
        let events = vec![event("2026-07-10T15:00:00Z", 500, 100, 20, 550)];
        assert!(reduce_codex_snapshot(&events, "acct", now).is_none());
    }

    #[test]
    fn no_events_at_all_reduce_to_none() {
        assert!(reduce_codex_snapshot(&[], "acct", ts("2026-07-10T20:30:00Z")).is_none());
    }
}
