use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

// CoinGecko's free tier throttles bursts hard, so each cycle sends one
// markets request plus at most CHART_BUDGET chart requests, spaced by
// CHART_GAP. Charts are cached and refreshed when older than CHART_TTL;
// the last good payload is re-emitted if a whole cycle fails.
const POLL: Duration = Duration::from_secs(120);
const CHART_TTL: Duration = Duration::from_secs(600);
const CHART_BUDGET: usize = 3;
const CHART_GAP: Duration = Duration::from_secs(3);
const COINS: [&str; 2] = ["bitcoin", "ethereum"];
const MARKETS_URL: &str = "https://api.coingecko.com/api/v3/coins/markets\
                           ?vs_currency=usd&ids=bitcoin,ethereum\
                           &price_change_percentage=24h,30d,1y";
// (label, market_chart days), indexed by period: 0 = day, 1 = month, 2 = year.
const PERIODS: [(&str, &str); 3] = [("1d", "1"), ("1m", "30"), ("1y", "365")];
// Refresh order: the widget defaults to the month view, so keep it freshest.
const PERIOD_PRIORITY: [usize; 3] = [1, 0, 2];
// Points kept per chart; enough for a mini sparkline.
const SPARK_POINTS: usize = 42;

#[derive(Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CryptoState {
    Disconnected { reason: String },
    Connected { coins: Vec<Coin> },
}

#[derive(Serialize, Clone, Default)]
struct Coin {
    symbol: String,
    price: f64,
    change_1d: f64,
    change_1m: f64,
    change_1y: f64,
    spark_1d: Vec<f64>,
    spark_1m: Vec<f64>,
    spark_1y: Vec<f64>,
    market_cap: f64,
    volume_24h: f64,
    high_24h: f64,
    low_24h: f64,
}

#[derive(Default)]
struct SparkSlot {
    data: Vec<f64>,
    fetched: Option<Instant>,
}

pub fn spawn(app: AppHandle, mut ready: tokio::sync::watch::Receiver<bool>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.wait_for(|r| *r).await;
        let client = match reqwest::Client::builder().user_agent("aria-hud").build() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("crypto: failed to build http client: {e}");
                return;
            }
        };
        let mut sparks: Vec<[SparkSlot; 3]> =
            COINS.iter().map(|_| Default::default()).collect();
        let mut last_ok: Option<Vec<Coin>> = None;
        loop {
            let state = match poll(&client, &mut sparks).await {
                Ok(coins) => {
                    last_ok = Some(coins.clone());
                    CryptoState::Connected { coins }
                }
                // Stale prices beat an error box; fall back to the last
                // good payload when the API throttles us.
                Err(reason) => match &last_ok {
                    Some(coins) => CryptoState::Connected { coins: coins.clone() },
                    None => CryptoState::Disconnected { reason },
                },
            };
            if let Err(e) = app.emit("crypto", state) {
                eprintln!("crypto emit failed: {e}");
            }
            // sparks/last_ok stay loop-local: a refresh wake only skips the
            // rest of the sleep, chart caches and budget logic are untouched.
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = REFRESH.notified() => {}
            }
        }
    });
}

/// Wakes the poll loop early (widget refresh button).
static REFRESH: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[tauri::command]
pub fn crypto_refresh() {
    REFRESH.notify_one();
}

async fn poll(
    client: &reqwest::Client,
    sparks: &mut [[SparkSlot; 3]],
) -> Result<Vec<Coin>, String> {
    let markets = get(client, MARKETS_URL).await?;
    let markets = markets
        .as_array()
        .ok_or_else(|| "unexpected response shape".to_string())?;

    refresh_charts(client, sparks).await;

    let mut coins = Vec::new();
    for (ci, id) in COINS.iter().enumerate() {
        let Some(m) = markets.iter().find(|m| m["id"] == *id) else {
            continue;
        };
        coins.push(Coin {
            symbol: m["symbol"].as_str().unwrap_or(id).to_uppercase(),
            price: m["current_price"].as_f64().unwrap_or(0.0),
            change_1d: pct(m, "price_change_percentage_24h_in_currency"),
            change_1m: pct(m, "price_change_percentage_30d_in_currency"),
            change_1y: pct(m, "price_change_percentage_1y_in_currency"),
            spark_1d: sparks[ci][0].data.clone(),
            spark_1m: sparks[ci][1].data.clone(),
            spark_1y: sparks[ci][2].data.clone(),
            market_cap: m["market_cap"].as_f64().unwrap_or(0.0),
            volume_24h: m["total_volume"].as_f64().unwrap_or(0.0),
            high_24h: m["high_24h"].as_f64().unwrap_or(0.0),
            low_24h: m["low_24h"].as_f64().unwrap_or(0.0),
        });
    }

    if coins.is_empty() {
        return Err("no market data returned".into());
    }
    Ok(coins)
}

/// Refresh up to CHART_BUDGET stale charts, month view first. A failed
/// fetch (rate limit) still consumes budget so we back off naturally.
async fn refresh_charts(client: &reqwest::Client, sparks: &mut [[SparkSlot; 3]]) {
    let mut budget = CHART_BUDGET;
    for pj in PERIOD_PRIORITY {
        for (ci, id) in COINS.iter().enumerate() {
            if budget == 0 {
                return;
            }
            let slot = &mut sparks[ci][pj];
            if slot.fetched.is_some_and(|t| t.elapsed() < CHART_TTL) {
                continue;
            }
            let url = format!(
                "https://api.coingecko.com/api/v3/coins/{id}/market_chart\
                 ?vs_currency=usd&days={}",
                PERIODS[pj].1
            );
            match get(client, &url).await {
                Ok(chart) => {
                    slot.data = downsample(chart["prices"].as_array().unwrap_or(&Vec::new()));
                    slot.fetched = Some(Instant::now());
                }
                Err(e) => eprintln!("crypto chart {id}/{}: {e}", PERIODS[pj].0),
            }
            budget -= 1;
            tokio::time::sleep(CHART_GAP).await;
        }
    }
}

/// Only the 24h key has a legitimate fallback (the non-currency variant);
/// falling back across periods would mislabel a day change as month/year.
fn pct(m: &serde_json::Value, key: &str) -> f64 {
    let v = m[key].as_f64();
    if key == "price_change_percentage_24h_in_currency" {
        return v
            .or_else(|| m["price_change_percentage_24h"].as_f64())
            .unwrap_or(0.0);
    }
    v.unwrap_or(0.0)
}

/// Thin a market_chart price series ([[ts, price], …]) to SPARK_POINTS
/// evenly spaced samples, always keeping the latest price.
fn downsample(prices: &[serde_json::Value]) -> Vec<f64> {
    let vals: Vec<f64> = prices.iter().filter_map(|p| p[1].as_f64()).collect();
    if vals.len() <= SPARK_POINTS {
        return vals;
    }
    let last = vals.len() - 1;
    (0..SPARK_POINTS)
        .map(|i| vals[i * last / (SPARK_POINTS - 1)])
        .collect()
}

async fn get(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}
