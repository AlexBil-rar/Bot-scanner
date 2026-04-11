use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://advanced-trade-ws.coinbase.com";
fn txt(s: String) -> Message { Message::Text(s.into()) }

const COINBASE_PAIRS: &[(&str, &str)] = &[
    ("BTC-USD", "BTC"), ("ETH-USD", "ETH"), ("SOL-USD", "SOL"),
    ("XRP-USD", "XRP"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[coinbase] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[coinbase] connected");
                let (mut write, mut read) = ws_stream.split();

                let product_ids: Vec<&str> = COINBASE_PAIRS.iter().map(|(p, _)| *p).collect();

                // Публичная подписка без авторизации
                let sub = json!({
                    "type": "subscribe",
                    "product_ids": product_ids,
                    "channel": "ticker"
                });

                if let Err(e) = write.send(txt(sub.to_string())).await {
                    error!("[coinbase] subscribe: {e}");
                    continue;
                }
                info!("[coinbase] subscribed {} pairs", COINBASE_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(t)) => {
                            let text = t.to_string();
                            if text.contains("error") {
                                warn!("[coinbase] server error: {}", &text[..text.len().min(200)]);
                            }
                            match parse_message(&text) {
                                Ok(Some(p)) => {
                                    let e = last.entry(p.symbol.clone()).or_insert((0.0, 0.0));
                                    if p.bid == e.0 && p.ask == e.1 { continue; }
                                    *e = (p.bid, p.ask);
                                    if tx.send(p).await.is_err() { return; }
                                }
                                Ok(None) => {}
                                Err(_) => {}
                            }
                        }
                        Ok(_) => {}
                        Err(e) => { warn!("[coinbase] ws: {e}"); break; }
                    }
                }
                warn!("[coinbase] disconnected, reconnecting...");
            }
            Err(e) => error!("[coinbase] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("channel").and_then(|c| c.as_str()) != Some("ticker") { return Ok(None); }
    let events = v.get("events").and_then(|e| e.as_array()).ok_or_else(|| anyhow::anyhow!("no events"))?;
    for event in events {
        let tickers = match event.get("tickers").and_then(|t| t.as_array()) { Some(t) => t, None => continue };
        for ticker in tickers {
            let product_id = ticker.get("product_id").and_then(|p| p.as_str()).unwrap_or("");
            let symbol = COINBASE_PAIRS.iter().find(|(k, _)| *k == product_id).map(|(_, s)| *s);
            let symbol = match symbol { Some(s) => s, None => continue };
            let bid: f64 = ticker.get("best_bid").and_then(|b| b.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
            let ask: f64 = ticker.get("best_ask").and_then(|a| a.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
            if bid <= 0.0 || ask <= 0.0 { continue; }
            let bid_size: f64 = ticker.get("best_bid_quantity").and_then(|b| b.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
            let ask_size: f64 = ticker.get("best_ask_quantity").and_then(|a| a.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
            return Ok(Some(MarketPrice {
                exchange: "coinbase".to_string(), symbol: symbol.to_string(),
                bid, ask, bid_size, ask_size, timestamp_ms: now_ms(), is_usdt: false,
            }));
        }
    }
    Ok(None)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
