//! `igrok usage` — local token/cost aggregation across all sessions.
//!
//! ccusage-style global statistics: scans `~/.igrok/sessions/**/updates.jsonl`
//! for the per-turn usage records the shell persists (input/output/cache
//! read/cache write/reasoning tokens, model calls, API duration, per-model
//! breakdown) and aggregates them by day × model, with cost estimates from
//! the models.dev catalog cache when a price is known.
//!
//! [LOCAL-DEV] fork feature; the data source is entirely local.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

#[derive(Debug, Args, Clone)]
pub struct UsageArgs {
    /// Aggregate per calendar day (default), per session, or per model only.
    #[arg(long, value_enum, default_value = "day")]
    by: UsageGroupBy,

    /// Only include records from the last N days (default: all).
    #[arg(long)]
    since: Option<u32>,

    /// Emit machine-readable JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
enum UsageGroupBy {
    Day,
    Session,
    Model,
}

/// One per-model tally.
#[derive(Debug, Default, Clone)]
struct ModelTally {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    reasoning_tokens: u64,
    model_calls: u64,
    api_ms: u64,
    /// Provider-reported cost for these turns (ccusage `costUSD` semantics),
    /// when the response carried one.
    self_reported_usd: f64,
    /// USD cost when every component has a known price, else `None`.
    cost_usd: Option<f64>,
}

impl ModelTally {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// Per-model prices from the models.dev cache, USD per **million** tokens.
#[derive(Debug, Default, Clone, Copy)]
struct Prices {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

pub async fn run(args: UsageArgs) -> Result<()> {
    let sessions_root = xai_grok_shell::util::grok_home::grok_home().join("sessions");
    let prices = load_model_prices().await;

    // group key -> (model -> tally)
    let mut groups: BTreeMap<String, BTreeMap<String, ModelTally>> = BTreeMap::new();
    let cutoff_ts = args.since.map(|days| {
        now_ts() - u64::from(days) * 86_400
    });

    for file in session_update_files(&sessions_root) {
        let session_id = file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let file = std::fs::File::open(&file)?;
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let ts = v.get("timestamp").and_then(|t| t.as_u64()).unwrap_or(0);
            if let Some(cutoff) = cutoff_ts
                && ts < cutoff
            {
                continue;
            }
            let Some(usage) = v
                .pointer("/params/update/usage")
                .and_then(|u| u.as_object())
            else {
                continue;
            };
            // The record's own usage is the turn total; modelUsage splits it
            // per model. Use the split when present, else the outer block.
            let self_reported_usd = usage
                .get("costUsdTicks")
                .or_else(|| usage.get("costUSDTicks"))
                .and_then(|t| t.as_i64())
                .map(|ticks| ticks as f64 / 1e10);
            if let Some(models) = usage
                .get("modelUsage")
                .and_then(|m| m.as_object())
                .filter(|m| !m.is_empty())
            {
                // Multi-model turns carry only a turn-level cost that cannot
                // be attributed per model — leave those to the price table.
                let attributable = models.len() == 1;
                for (model, mv) in models {
                    let key = group_key(&args, ts, &session_id);
                    tally_entry(&mut groups, key.clone(), model, mv);
                    if attributable
                        && let Some(usd) = self_reported_usd
                        && let Some(tally) = groups
                            .get_mut(&key)
                            .and_then(|m| m.get_mut(model.as_str()))
                    {
                        tally.self_reported_usd += usd;
                    }
                }
            } else {
                let model = usage
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown");
                let outer = serde_json::Value::Object(usage.clone());
                let key = group_key(&args, ts, &session_id);
                tally_entry(&mut groups, key.clone(), model, &outer);
                if let Some(usd) = self_reported_usd
                    && let Some(tally) = groups
                        .get_mut(&key)
                        .and_then(|m| m.get_mut(model))
                {
                    tally.self_reported_usd += usd;
                }
            }
        }
    }

    // cost pass (needs the completed tallies): ccusage `auto` semantics —
    // a provider-reported cost wins over the price table.
    for models in groups.values_mut() {
        for (model, tally) in models.iter_mut() {
            tally.cost_usd = if tally.self_reported_usd > 0.0 {
                Some(tally.self_reported_usd)
            } else {
                estimate_cost(&prices, model, tally)
            };
        }
    }

    if groups.is_empty() {
        println!("No usage records found under {}", sessions_root.display());
        return Ok(());
    }
    if args.json {
        print_json(&groups);
    } else {
        print_table(&groups, args.by);
    }
    Ok(())
}

fn group_key(args: &UsageArgs, ts: u64, session_id: &str) -> String {
    match args.by {
        UsageGroupBy::Day => local_date(ts),
        UsageGroupBy::Session => session_id.to_string(),
        UsageGroupBy::Model => "all".to_string(),
    }
}

/// Accumulate one usage record into `groups[group][model]`.
fn tally_entry(
    groups: &mut BTreeMap<String, BTreeMap<String, ModelTally>>,
    group: String,
    model: &str,
    src: &serde_json::Value,
) {
    let tally = groups
        .entry(group)
        .or_default()
        .entry(model.to_string())
        .or_default();
    tally.input_tokens += get_u64(src, "inputTokens");
    tally.output_tokens += get_u64(src, "outputTokens");
    tally.cache_read_tokens += get_u64(src, "cachedReadTokens");
    tally.cache_write_tokens += get_u64(src, "cacheCreationTokens");
    tally.reasoning_tokens += get_u64(src, "reasoningTokens");
    tally.model_calls += get_u64(src, "modelCalls");
    tally.api_ms += get_u64(src, "apiDurationMs");
}

/// Local-timezone `YYYY-MM-DD` from a unix timestamp (via chrono, which the
/// pager already depends on).
fn local_date(ts: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts as i64, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn get_u64(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn session_update_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(cwds) = std::fs::read_dir(root) else {
        return out;
    };
    for cwd in cwds.flatten() {
        let Ok(sessions) = std::fs::read_dir(cwd.path()) else {
            continue;
        };
        for session in sessions.flatten() {
            let updates = session.path().join("updates.jsonl");
            if updates.is_file() {
                out.push(updates);
            }
        }
    }
    out.sort();
    out
}

// ── pricing ─────────────────────────────────────────────────────────────────

/// models.dev catalog URL + cache TTL, shared with the shell's dev-catalog
/// refresh (same cache file, so a TUI session keeps this fresh for free).
const MODELS_DEV_API: &str = "https://models.dev/api.json";
const PRICE_CACHE_TTL_SECS: u64 = 24 * 60 * 60;

fn price_cache_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("models_dev_catalog.json")
}

/// `model id -> prices`, ccusage-style: fresh cache first, one network fetch
/// when stale/missing (stored back into the shared cache), stale-cache
/// fallback offline.
async fn load_model_prices() -> BTreeMap<String, Prices> {
    let path = price_cache_path();
    let cached = std::fs::read_to_string(&path)
        .ok()
        .and_then(|txt| serde_json::from_str::<serde_json::Value>(&txt).ok().map(|c| (txt, c)));
    let fresh = cached.as_ref().is_some_and(|(_, c)| {
        c.get("fetched_at")
            .and_then(|t| t.as_u64())
            .is_some_and(|at| now_ts().saturating_sub(at) < PRICE_CACHE_TTL_SECS)
    });
    if !fresh {
        eprintln!("Fetching model prices from models.dev …");
        if let Ok(body) = fetch_models_dev().await {
            let stamped = serde_json::json!({ "fetched_at": now_ts(), "raw": body });
            let _ = std::fs::write(&path, stamped.to_string());
            return parse_prices(&body);
        }
        if cached.is_none() {
            eprintln!("(offline and no cached price catalog — costs will show '-')");
        }
    }
    // fresh cache, or stale fallback offline
    match cached {
        Some((_, c)) => {
            let raw = c.get("raw").and_then(|r| r.as_str()).unwrap_or("");
            parse_prices(raw)
        }
        None => BTreeMap::new(),
    }
}

async fn fetch_models_dev() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let resp = client.get(MODELS_DEV_API).send().await?.error_for_status()?;
    Ok(resp.text().await?)
}

fn parse_prices(raw: &str) -> BTreeMap<String, Prices> {
    let mut map = BTreeMap::new();
    let Ok(catalog) = serde_json::from_str::<serde_json::Value>(raw) else {
        return map;
    };
    let Some(providers) = catalog.as_object() else {
        return map;
    };
    for (_pkey, provider) in providers {
        let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
            continue;
        };
        for (mkey, model) in models {
            let Some(cost) = model.get("cost").and_then(|c| c.as_object()) else {
                continue;
            };
            let p = Prices {
                input: cost.get("input").and_then(|x| x.as_f64()),
                output: cost.get("output").and_then(|x| x.as_f64()),
                cache_read: cost
                    .get("cached_input")
                    .or_else(|| cost.get("cache_read"))
                    .and_then(|x| x.as_f64()),
                cache_write: cost
                    .get("input_cache_write")
                    .or_else(|| cost.get("cache_write"))
                    .and_then(|x| x.as_f64()),
            };
            map.insert(mkey.clone(), p);
        }
    }
    map
}

fn estimate_cost(prices: &BTreeMap<String, Prices>, model: &str, t: &ModelTally) -> Option<f64> {
    let p = prices.get(model)?;
    let per = |usd_pm: Option<f64>, tokens: u64| -> Option<f64> {
        usd_pm.map(|pm| pm * tokens as f64 / 1_000_000.0)
    };
    // Partial prices price only what they know (models.dev convention).
    let mut usd = 0.0;
    let mut any = false;
    if let Some(c) = per(p.input, t.input_tokens) {
        usd += c;
        any = true;
    }
    if let Some(c) = per(p.output, t.output_tokens) {
        usd += c;
        any = true;
    }
    if let Some(c) = per(p.cache_read, t.cache_read_tokens) {
        usd += c;
        any = true;
    }
    if let Some(c) = per(p.cache_write, t.cache_write_tokens) {
        usd += c;
        any = true;
    }
    any.then_some(usd)
}

// ── output ──────────────────────────────────────────────────────────────────

fn print_table(groups: &BTreeMap<String, BTreeMap<String, ModelTally>>, by: UsageGroupBy) {
    let (group_label, group_w) = match by {
        UsageGroupBy::Day => ("DAY", 12usize),
        UsageGroupBy::Session => ("SESSION", 20),
        UsageGroupBy::Model => ("MODEL", 24),
    };
    let model_w: usize = if by == UsageGroupBy::Model { 0 } else { 24 };
    let mut grand = ModelTally {
        cost_usd: Some(0.0),
        ..ModelTally::default()
    };
    let mut grand_cost_known = true;
    if model_w > 0 {
        println!(
            "{:<gw$} {:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
            group_label,
            "MODEL",
            "INPUT",
            "OUTPUT",
            "CACHE-R",
            "CACHE-W",
            "CALLS",
            "COST",
            gw = group_w
        );
    } else {
        println!(
            "{:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
            "MODEL",
            "INPUT",
            "OUTPUT",
            "CACHE-R",
            "CACHE-W",
            "CALLS",
            "COST"
        );
    }
    for (group, models) in groups {
        for (model, t) in models {
            let cost = match t.cost_usd {
                Some(c) => format!("${c:.2}"),
                None => {
                    grand_cost_known = false;
                    "-".to_string()
                }
            };
            if model_w > 0 {
                println!(
                    "{:<gw$} {:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
                    truncate(group, group_w),
                    truncate(model, 24),
                    fmt_tokens(t.input_tokens),
                    fmt_tokens(t.output_tokens),
                    fmt_tokens(t.cache_read_tokens),
                    fmt_tokens(t.cache_write_tokens),
                    t.model_calls,
                    cost,
                    gw = group_w
                );
            } else {
                println!(
                    "{:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
                    truncate(model, 24),
                    fmt_tokens(t.input_tokens),
                    fmt_tokens(t.output_tokens),
                    fmt_tokens(t.cache_read_tokens),
                    fmt_tokens(t.cache_write_tokens),
                    t.model_calls,
                    cost
                );
            }
            grand.input_tokens += t.input_tokens;
            grand.output_tokens += t.output_tokens;
            grand.cache_read_tokens += t.cache_read_tokens;
            grand.cache_write_tokens += t.cache_write_tokens;
            grand.reasoning_tokens += t.reasoning_tokens;
            grand.model_calls += t.model_calls;
            grand.api_ms += t.api_ms;
            if let Some(c) = t.cost_usd
                && let Some(g) = grand.cost_usd.as_mut()
            {
                *g += c;
            }
        }
    }
    let total_cost = if grand_cost_known {
        format!("${:.2}", grand.cost_usd.unwrap_or(0.0))
    } else {
        format!("${:.2}+", grand.cost_usd.unwrap_or(0.0))
    };
    println!("{}", "-".repeat(96));
    if model_w > 0 {
        println!(
            "{:<gw$} {:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
            "TOTAL",
            "",
            fmt_tokens(grand.input_tokens),
            fmt_tokens(grand.output_tokens),
            fmt_tokens(grand.cache_read_tokens),
            fmt_tokens(grand.cache_write_tokens),
            grand.model_calls,
            total_cost,
            gw = group_w
        );
    } else {
        println!(
            "{:<24} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
            "TOTAL",
            fmt_tokens(grand.input_tokens),
            fmt_tokens(grand.output_tokens),
            fmt_tokens(grand.cache_read_tokens),
            fmt_tokens(grand.cache_write_tokens),
            grand.model_calls,
            total_cost
        );
    }
    if grand.api_ms > 0 && grand.model_calls > 0 {
        println!(
            "\nAPI time: {} across {} calls (avg {:.1}s/call)",
            fmt_duration(grand.api_ms),
            grand.model_calls,
            grand.api_ms as f64 / 1000.0 / grand.model_calls as f64
        );
    }
}

fn print_json(groups: &BTreeMap<String, BTreeMap<String, ModelTally>>) {
    use serde_json::json;
    let mut out = serde_json::Map::new();
    for (group, models) in groups {
        let models_json: serde_json::Map<String, serde_json::Value> = models
            .iter()
            .map(|(model, t)| {
                (
                    model.clone(),
                    json!({
                        "inputTokens": t.input_tokens,
                        "outputTokens": t.output_tokens,
                        "cacheReadTokens": t.cache_read_tokens,
                        "cacheWriteTokens": t.cache_write_tokens,
                        "reasoningTokens": t.reasoning_tokens,
                        "totalTokens": t.total_tokens(),
                        "modelCalls": t.model_calls,
                        "apiDurationMs": t.api_ms,
                        "costUSD": t.cost_usd,
                    }),
                )
            })
            .collect();
        out.insert(group.clone(), serde_json::Value::Object(models_json));
    }
    let Ok(pretty) = serde_json::to_string_pretty(&out) else {
        return;
    };
    println!("{pretty}");
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end against the local `~/.igrok/sessions` tree (empty on CI:
    /// asserts the no-records path). Run with --nocapture to eyeball the
    /// table on a machine with real sessions.
    #[tokio::test]
    async fn usage_run_day_ok() {
        let args = UsageArgs {
            by: UsageGroupBy::Day,
            since: None,
            json: false,
        };
        assert!(run(args).await.is_ok());
    }

    #[tokio::test]
    async fn usage_run_session_json_ok() {
        let args = UsageArgs {
            by: UsageGroupBy::Session,
            since: Some(30),
            json: true,
        };
        assert!(run(args).await.is_ok());
    }

    #[test]
    fn token_formatting() {
        assert_eq!(fmt_tokens(732), "732");
        assert_eq!(fmt_tokens(15664), "15.7k");
        assert_eq!(fmt_tokens(1_564_000), "1.56M");
    }

    #[test]
    fn cost_estimation_partial_prices() {
        let mut prices = BTreeMap::new();
        prices.insert(
            "test-model".to_string(),
            Prices {
                input: Some(3.0),
                output: Some(15.0),
                cache_read: Some(0.3),
                cache_write: Some(3.75),
            },
        );
        let t = ModelTally {
            input_tokens: 1_000_000,
            output_tokens: 2_000_000,
            cache_read_tokens: 4_000_000,
            cache_write_tokens: 0,
            ..ModelTally::default()
        };
        let usd = estimate_cost(&prices, "test-model", &t).unwrap();
        // 3*1 + 15*2 + 0.3*4 = 22.2
        assert!((usd - 22.2).abs() < 1e-6);
        assert!(estimate_cost(&prices, "unknown-model", &t).is_none());
    }
}
