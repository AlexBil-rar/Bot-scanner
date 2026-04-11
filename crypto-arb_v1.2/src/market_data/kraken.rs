use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://ws.kraken.com/v2";
fn txt(s: String) -> Message { Message::Text(s.into()) }

// Kraken торгует XBT вместо BTC, BNB не торгуется на Kraken
const KRAKEN_PAIRS: &[(&str, &str)] = &[
    ("BTC/USD", "BTC"),
    ("ETH/USD", "ETH"),
    ("SOL/USD", "SOL"),
    ("XRP/USD", "XRP"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[kraken] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[kraken] connected");
                let (mut write, mut read) = ws_stream.split();

                let symbols: Vec<&str> = KRAKEN_PAIRS.iter().map(|(s, _)| *s).collect();
                let sub = json!({"method":"subscribe","params":{"channel":"ticker","symbol": symbols}});
                if let Err(e) = write.send(txt(sub.to_string())).await {
                    error!("[kraken] subscribe failed: {e}"); continue;
                }
                info!("[kraken] subscribed {} pairs", KRAKEN_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(t)) => {
                            match parse_message(&t.to_string()) {
                                Ok(Some(p)) => {
                                    let e = last.entry(p.symbol.clone()).or_insert((0.0, 0.0));
                                    if p.bid == e.0 && p.ask == e.1 { continue; }
                                    *e = (p.bid, p.ask);
                                    if tx.send(p).await.is_err() { return; }
                                }
                                Ok(None) => {}
                                Err(e) => warn!("[kraken] parse: {e}"),
                            }
                        }
                        Ok(_) => {}
                        Err(e) => { warn!("[kraken] ws: {e}"); break; }
                    }
                }
                warn!("[kraken] disconnected, reconnecting...");
            }
            Err(e) => error!("[kraken] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("channel").and_then(|c| c.as_str()) != Some("ticker") { return Ok(None); }
    if v.get("type").and_then(|t| t.as_str()).map(|t| t != "update" && t != "snapshot").unwrap_or(true) { return Ok(None); }

    let data = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("no data"))?;

    let ks = data.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
    let symbol = KRAKEN_PAIRS.iter().find(|(k, _)| *k == ks).map(|(_, s)| *s);
    let symbol = match symbol { Some(s) => s, None => return Ok(None) };

    let bid = data.get("bid").and_then(|b| b.as_f64()).ok_or_else(|| anyhow::anyhow!("no bid"))?;
    let ask = data.get("ask").and_then(|a| a.as_f64()).ok_or_else(|| anyhow::anyhow!("no ask"))?;
    let bid_size = data.get("bid_qty").and_then(|b| b.as_f64()).unwrap_or(0.0);
    let ask_size = data.get("ask_qty").and_then(|a| a.as_f64()).unwrap_or(0.0);

    Ok(Some(MarketPrice { exchange: "kraken".to_string(), symbol: symbol.to_string(), bid, ask, bid_size, ask_size, timestamp_ms: now_ms(), is_usdt: false }))
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
