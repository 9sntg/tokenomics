# tokenomics — Feasibility & Architecture Research

> Goal: a TUI that monitors multiple LLM **subscription** accounts — today 3× Claude Max
> (a personal, a work, and a project account), later
> OpenAI Codex and Google Gemini — showing **usage / spend, limits, and time-left**.
>
> Research date: 2026-07-04. Method: 16-agent workflow (6 research lanes → adversarial
> verification → 3 architecture proposals → synthesis). Several claims were **live-verified**
> against the machine and the network on the research date; treat undocumented endpoints as
> drift-prone.

---

## 1. Bottom line

**Feasible and genuinely useful — if you refuse to conflate two data planes:**

1. **Token/usage from local logs = the reliable, ToS-safe core.** Claude Code writes every
   message's token usage to `~/.claude/projects/**/*.jsonl`. This is what `ccusage` reads. No
   network, no credentials, no terms-of-service exposure. Buildable in ~a day.

2. **Authoritative rate-limit state (real 5h + weekly utilization % and reset timestamps) =
   an opt-in overlay, not the foundation.** It exists — `GET https://api.anthropic.com/api/oauth/usage`
   returned HTTP 200 on the research date (five_hour 19%, seven_day 57%) — but it is
   **undocumented, reverse-engineered, aggressively 429-throttled, and a by-the-letter
   violation of Anthropic's 2026 consumer-OAuth terms.** So it must be a labeled, opt-in
   feature that degrades gracefully to local estimates.

**The honestly hard / impossible parts:**

- **Account attribution is impossible from logs alone.** JSONL carries zero account/email
  identity; `~/.claude.json` holds only the *current* login and is overwritten on switch. The
  only way to separate 3 accounts is to run each under its own `CLAUDE_CONFIG_DIR`.
- **There is no dollar "spend" on a flat-fee subscription** — only a *notional* API-list-price
  equivalent (what ccusage prints) and **% window utilization**. Never present it as a bill.
- **No provider exposes an absolute ceiling.** You show **"% used + reset countdown"**, never
  "X of Y left" — any absolute number would be a drifting third-party guess.
- **Weekly limits + true remaining-% are server-side only** — invisible without the overlay.

---

## 2. Feasibility matrix

Legend: ✅ yes · 🟡 partial · ❌ no/none. Confidence reflects the adversarial-verification pass.

### Claude Max (the 3 accounts today)

| Capability | Verdict | Data source | Confidence |
|---|---|---|---|
| **Token usage** | ✅ (dollars are *notional* only) | `~/.claude/projects/**/*.jsonl` `message.usage`, via `ccusage --json` or direct parse. **Must sum `input + cache_read + cache_creation`** (raw `input_tokens` ≈ 0 due to caching). | **High** |
| **Limits (5h + weekly %)** | 🟡 authoritative via the overlay endpoint; otherwise only a derived 5h token reconstruction (no true %), weekly not derivable locally | `GET /api/oauth/usage` → `limits[]` + `five_hour`/`seven_day`/`seven_day_opus`/`seven_day_sonnet` utilization%. Fallback: 5h-block reconstruction from JSONL timestamps. | **Medium** (undocumented, 429-prone, ToS-gray) |
| **Time-left / reset** | ✅ authoritative windows; degraded 5h estimate without overlay; **weekly reset has no local fallback** | `resets_at` (ISO8601 — **render verbatim, never recompute**) from the endpoint. | **Medium** |
| **Multi-account attribution** | 🟡 impossible from logs; solvable via isolation | **`CLAUDE_CONFIG_DIR` per account** (each dir has its own `.credentials.json`). NOT the logs. | **High** (that logs can't; that config-dir works) |

### Codex (OpenAI — installed but not logged in here)

| Capability | Verdict | Data source | Confidence |
|---|---|---|---|
| **Token usage** | ✅ per-turn + cumulative | `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` `token_count` events. | **Med-High** (schema confirmed; untested locally) |
| **Limits (5h + weekly %)** | ✅ real server values, with caveats | Same events' `rate_limits` (`RateLimitSnapshot` primary=5h / secondary=weekly `used_percent` + `plan_type`); or spawn `codex app-server` JSON-RPC `account/rateLimits/read`. | **Medium** (exec-mode emits `null` rate_limits — issue #14728; stale until CLI next hits server; no shipped `codex status --json` — issue #20310) |
| **Time-left / reset** | ✅ absolute `resets_at` | `RateLimitWindow.resets_at` (epoch) in the rollout. | **Medium** |
| **Multi-account** | 🟡 via `CODEX_HOME` isolation (inferred) | Separate `$CODEX_HOME` per account. | **Low** (inferred, unverified) |

> Codex is **non-functional in this WSL today** (Windows npm build, no Linux binary, empty
> `sessions/`, only `hooks.json`). Needs a working Linux `codex` + ChatGPT login before any
> data appears. It is, however, the **best-instrumented** non-Claude provider.

### Gemini (Google — not installed here; weakest)

| Capability | Verdict | Data source | Confidence |
|---|---|---|---|
| **Token usage** | 🟡 estimate/counting only | opt-in OTEL `gemini_cli.token.usage` (telemetry OFF by default) **or** `~/.gemini/tmp/*/chats/session-*.json` token sums. | **Low** |
| **Limits (%)** | ❌ no authoritative surface | Only static per-tier request caps (60rpm/1000rpd personal; 1500/day AI Pro; 2000/day Ultra). | **Low** |
| **Time-left / reset** | ❌ none exposed | — (request-count daily/minute quotas; could only infer daily rollover). | **Low (N/A)** |
| **Multi-account** | 🟡 per-home isolation, but moot without usage data | separate home dir per account. | **Low** |

> Gemini gives almost nothing for free and **may be deprecating toward the Antigravity CLI in
> 2026** — verify before investing. Any Gemini panel is a usage *estimate*, never a real limit.

### Documented negative (don't chase it)

The Anthropic **Admin** `/v1/organizations/usage_report`+`cost_report` and OpenAI
`/v1/organization/usage`+`costs` are **org-admin-key metered-API** endpoints. They **do not
cover Max/Plus/Codex subscription accounts** — a red herring for these 3 accounts. Only relevant
if a real API-key account is later added.

---

## 3. The concrete mechanisms

### 3.1 Local JSONL logs (primary, ToS-safe)
- Path: `~/.claude/projects/**/*.jsonl` (per `CLAUDE_CONFIG_DIR`). Here: 766 files / 260 MB.
- Per-message `message.usage`: `input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
  `cache_read_input_tokens`, `service_tier`, `cache_creation.{ephemeral_1h/5m}`, `iterations[]`;
  top-level `model`, `timestamp`, `requestId`, `sessionId`, `cwd`, `version`. **No `costUSD`**
  (present through Claude Code ~v1.0.6, gone by ~v1.0.9 / June 2025 — ccusage computes cost from
  a LiteLLM pricing table).
- **Token-math trap:** `input_tokens` is ~0 due to caching (median 2 on this machine) and
  `output_tokens` excludes billed thinking tokens. Any homemade sum must add
  `cache_read + cache_creation` or it undercounts real context ~100×. ccusage already does this.
- **ccusage is now a native Rust binary** (`v20.0.14` here) with **no JS exports** — you **cannot
  import it**; shell out with `--json`. (Importable only ≤ v18 for `ccusage/data-loader` +
  `ccusage/calculate-cost`; `blocks --live` was removed in v18.) `ccusage blocks --json`
  emits per-block `tokenCounts`, `costUSD`, `models[]`, `start/end/actualEnd`, `isActive`,
  `burnRate`, `projection` — exactly the panel fields. **Pin a version** to avoid schema drift.
- ccusage dedupes by `messageId:requestId` keeping the **largest-total-tokens** entry.

### 3.2 `CLAUDE_CONFIG_DIR` isolation (the only multi-account path)
- Each account logs in once under its own config dir → its own `~/.claude` tree + own
  `.credentials.json`. The collector iterates the account list, setting `CLAUDE_CONFIG_DIR`
  per ccusage invocation / per overlay poll. Attribution comes from **the dir, never the logs**.
- On this WSL/Linux box credentials are a flat `0600` file per dir (verified). Keep dirs on
  Linux ext4 — a `/mnt/c` Windows mount can make `.credentials.json` world-readable.

### 3.3 The authoritative limits overlay (opt-in, ToS-gray)
- `GET https://api.anthropic.com/api/oauth/usage`
  - Auth: `Authorization: Bearer <accessToken>` read from `<configDir>/.credentials.json` →
    `claudeAiOauth.accessToken`. (Verified structure: `claudeAiOauth = { accessToken,
    refreshToken, expiresAt, scopes[5], subscriptionType, rateLimitTier }`.)
  - Headers: `anthropic-beta: oauth-2025-04-20` and `User-Agent: claude-code/<ver>`. **Not
    strictly mandatory** — single requests returned 200 with the beta header omitted and with a
    wrong UA — **but** the *old* `claude-cli/<ver>` UA now 404s, so send the current
    `claude-code/<ver>`.
  - Response: `limits[]` (objects `{kind, group, percent, severity, resets_at, scope}`) plus
    `five_hour` / `seven_day` / `seven_day_opus` / `seven_day_sonnet` — **utilization % + `resets_at`
    only; all `*_dollars`/ceiling fields are `null`** for subscription windows.
  - **Throttling is real:** it 429s with **no `Retry-After`**, and the "~180s is safe" figure is
    **unverified community lore** — issue #31637 documents 429s even at 30–60s. Poll conservatively,
    back off hard, cache last-good, degrade to derived estimates.
- **Token refresh** (only if you run 24/7): `POST grant_type=refresh_token`, `client_id
  9d1c250a-e61b-44d9-88ed-5944d1962f5e`, `Content-Type: x-www-form-urlencoded` to
  `platform.claude.com/v1/oauth/token` (legacy `console.anthropic.com` mid-migration — resolve host
  at runtime). **Cloudflare WAF challenges/403s headless requests on WSL**; refresh tokens
  **rotate** (persist atomically or you desync the CLI). Isolate config dirs so a live CLI and the
  monitor don't refresh-race. Access tokens are short-lived (~8–24h).
- **ToS-clean alternatives:** (a) the **official statusline** stdin JSON carries
  `rate_limits.five_hour/seven_day.{used_percentage, resets_at}` — but only during a **live
  in-repo session**, one account at a time (can't poll idle accounts). Capture opportunistically.
  (b) `anthropic-ratelimit-unified-*` **response headers** on real Messages API calls — but that
  requires making a consuming request.

### 3.4 Codex & Gemini (future adapters)
- **Codex:** parse `$CODEX_HOME/sessions/.../rollout-*.jsonl` `token_count` events → usage +
  `rate_limits` (primary 5h / secondary weekly `used_percent` + `resets_at` + `plan_type`). No
  shipped `codex status --json` (issue #20310); `rate_limits` is `null` in exec-mode (#14728) and
  stale until the CLI next hits the server. `codex app-server` JSON-RPC `account/rateLimits/read`
  is an alternative. `CODEX_HOME` per account for isolation. (`CodexBar` community tool already
  does multi-account Codex.)
- **Gemini:** opt-in OTEL or session-file token sums — **estimate only, no reset, no quota API.**

---

## 4. Recommended architecture

**Local-first TUI on a provider-adapter spine, with authoritative limits as a labeled opt-in
overlay.** (Principled merge: fast local-JSONL base + a clean adapter/contract seam + the
endpoint *demoted* from headline feature to opt-in overlay.)

### Why this shape
- **Lead with the ToS-safe local path** so a genuinely useful single-account dashboard ships
  **day 1** with almost no parsing code (`ccusage blocks --json` already returns every panel field).
- **Build on normalized contracts + a `ProviderAdapter` interface** so Codex/Gemini are additive
  plug-ins, not a rewrite, and a future web view reads the same store unchanged.
- **Demote the OAuth endpoint to a config-gated, provenance-tagged overlay** that you consciously
  opt into and that degrades silently to derived 5h estimates on 429/breakage — because making the
  tool's core value depend on an undocumented, throttled, ToS-by-the-letter-violating endpoint plus
  fragile headless OAuth refresh is the wrong risk posture.
- **Rejected alternatives:** *limits-first* (Proposal C) makes the headline value hinge on the most
  fragile, legally sensitive piece (1.5–2.5 wk MVP, breakage/flag risk); *daemon-first extensibility*
  (Proposal B) pays the daemon + multi-package + systemd + migrations tax upfront for one user / 3
  accounts. We keep B's good bones (adapter seam, `UsageSnapshot`/`Limit` contracts with a provenance
  flag, SQLite with byte-offset cursors) but defer the daemon until multi-account needs it.

### Contracts (the extensibility spine)
```ts
type UsageSnapshot = {
  account_id: string; provider: 'claude' | 'codex' | 'gemini';
  collected_at: string;
  input: number; output: number; cache_read: number; cache_creation: number;
  total_tokens: number; cost_notional?: number;   // labeled a usage proxy, never a bill
};
type Limit = {
  account_id: string; provider: string;
  kind: 'session' | 'weekly_all' | 'weekly_scoped'; scope?: string;
  utilization_pct: number;          // 0–100 — never an absolute count
  resets_at: string;                // ISO8601, rendered verbatim
  severity: 'ok' | 'warn' | 'crit';
  source: 'authoritative' | 'derived' | 'estimate';   // provenance badge in the UI
};
interface ProviderAdapter {
  id: string;
  listAccounts(): AccountRef[];
  collect(a: AccountRef): Promise<UsageSnapshot>;
  fetchLimits?(a: AccountRef): Promise<Limit[]>;
}
```

### Components
- `config` — `tokenomics.(json|toml)`: `accounts[] = {label, provider, configDir, color}`, thresholds,
  per-account `limitsOverlay` on/off. Single source of the account list.
- `collector/ccusageSource` — `execFile ccusage blocks --json` with `CLAUDE_CONFIG_DIR` per account.
- `collector/jsonlParser` — independent fallback: incremental byte-offset tail, dedupe
  `messageId:requestId`, sum cache fields, reconstruct 5h blocks.
- `store/db` — `better-sqlite3` (WAL): `snapshots`, `blocks`, `limits`, `file_cursors(path,offset,mtime)`,
  `heartbeat`. Cursors make polling O(new-bytes), not a re-parse of 260 MB per frame.
- `limits/oauthUsage` — **opt-in, config-gated.** Poll the endpoint, write `source:'authoritative'`
  Limit rows; on any error fall back to `source:'derived'`. Never crashes the collector.
- `providers/*` — `claude` ships first; `codex`, `gemini` added later without touching the TUI.
- `tui/*` — per-account panels: gauge (% window), provenance badge, Unicode-block sparkline
  (token burn), live `resets_at` countdown, cost-as-proxy label, threshold alert banner.

### Stack
- **Runtime:** Node 25 + TypeScript (or Python 3.14 — see the open TUI-framework decision).
- **TUI:** **Ink 7** (React/Yoga, flicker-free frame diff) + `@inkjs/ui` (ProgressBar/Badge/Alert) +
  a ~30-line hand-rolled Unicode-block sparkline. **Alternative: Textual** (Python) has *first-party*
  `ProgressBar` **and** `Sparkline` widgets + timers — lowest wiring, and since ccusage is shelled
  out regardless, the "stay in Node" argument is weaker than it first looks.
- **Data engine:** `ccusage` v20 CLI via `child_process.execFile --json` (shell out; do **not**
  import), plus a direct JSONL parser as independent fallback.
- **Store:** `better-sqlite3` (synchronous, WAL) — collector writes, TUI reads.
- **Run model:** collector + TUI in one process on day 1 (`setInterval`); split to a detached
  collector (systemd user unit / pm2) once multi-account + overlay land.
- **Alerts:** in-TUI banner = source of truth; `node-notifier` best-effort (WSL2 desktop
  notifications are flaky).

---

## 5. Build plan

| Phase | Deliverable | Effort |
|---|---|---|
| **1 — Single-account local dashboard** (useful minimal slice) | Ink + `@inkjs/ui` scaffold; `config`; `ccusageSource` shelling `ccusage blocks --json` with `CLAUDE_CONFIG_DIR`; one panel: token burn, notional cost (labeled a proxy), current 5h token total, reconstructed time-left. In-process poll, no DB, **zero network / zero OAuth use.** | ~1 day |
| **2 — Multi-account + persistence + provider seam** | `better-sqlite3` collector/store split with byte-offset cursors; iterate all 3 `CLAUDE_CONFIG_DIR`s; per-account panels + sparkline; `UsageSnapshot`/`Limit` contracts + `ProviderAdapter`; direct-JSONL fallback. | ~1–2 days |
| **3 — Authoritative limits overlay (OPT-IN) + alerts** | Config-gated `/api/oauth/usage` poller (hard 429 backoff, correct UA, token per account); provenance-tagged Limit rows; UI badges + real reset countdowns; threshold banner + best-effort desktop notify; detach collector as systemd unit; optional token-refresh loop only if 24/7. | ~1–2 days + upkeep |
| **4 — Multi-provider** | Codex adapter (needs Linux `codex` + ChatGPT login + `CODEX_HOME` isolation). Gemini adapter (estimate-only, low priority). Optional `bun build --compile` single binary (validate `better-sqlite3` native-addon bundling). | ~2–4 days/provider |

---

## 6. Risks (carry these into the build)

1. **Attribution unsolvable from logs** — requires the per-`CLAUDE_CONFIG_DIR` setup (each account
   logged in once). Without it the tool physically can't separate accounts.
2. **Overlay endpoint is undocumented/reverse-engineered** — path/schema/headers can change or be
   gated without notice; sticky 429s with no `Retry-After`. Degrade gracefully from day one.
3. **ToS:** consumer OAuth tokens are authorized only for Claude Code + Claude.ai (2026 terms,
   server-side enforcement on *inference*). A read-only usage poll is a by-the-letter violation and is
   fingerprintable — account flag/revocation is *possible* (not observed yet for usage-only polling).
   Make the overlay strictly opt-in; local JSONL is the safe default.
4. **"Spend" is notional** — frame as usage proxy / % utilization, never a bill. New model ids (e.g.
   `claude-opus-4-8`) can be mispriced/$0 until pricing tables catch up.
5. **Token-math** — must sum `cache_read + cache_creation`; `output_tokens` excludes thinking tokens.
6. **No absolute ceilings** anywhere — gauges are "% used + countdown", never "X of Y left".
7. **Weekly limits + true %** are server-side only; without the overlay they're invisible.
8. **Headless OAuth refresh is fragile** — Cloudflare WAF, host migration, rotating refresh tokens,
   refresh-race with a live CLI. Isolate dirs, persist atomically, fall back to interactive re-login.
9. **ccusage v20 can't be imported** — shell out; pin a version; tolerate flag/schema drift.
10. **Codex non-functional in this WSL**; **Gemini gives almost nothing** and may be deprecating.

---

## 7. Open decisions (these gate the build)

1. **Machine topology** — do all 3 Max accounts run on *this* machine (→ one `CLAUDE_CONFIG_DIR`
   each, single local collector) or across machines (→ collector per machine into a shared store)?
2. **ToS comfort** — willing to poll `/api/oauth/usage` for authoritative 5h/weekly %+reset (gray-zone,
   small flag risk), or local-JSONL-only (token usage + reconstructed 5h, no true %, no weekly)?
   *This single choice decides whether Phase 3 happens at all.*
3. **Weekly limits** — do you actually need authoritative weekly %/reset (overlay-only, no local
   fallback), or is the 5h window enough?
4. **Run pattern** — 24/7 unattended (forces the fragile headless refresh loop) or launched on demand
   while already logged in (sidesteps most refresh pain)?
5. **Pay-as-you-go overage** — is real-money overage enabled on any account? That's the *only* real
   dollar figure on a subscription and worth surfacing separately.
6. **Codex/Gemini priority** — install a working Linux `codex` + ChatGPT login now so its adapter is
   testable (strongest non-Claude provider)? Build a Gemini panel at all given no quota surface?
7. **TUI language + distribution** — Ink (TS, matches your stack) vs Textual (Python, batteries-included
   widgets) vs Bubble Tea (Go, single binary)? npx/local vs a compiled single binary?
