use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";
fn txt(s: String) -> Message { Message::Text(s.into()) }

const OKX_PAIRS: &[(&str, &str)] = &[
    ("BTC-USDT", "BTC"), ("ETH-USDT", "ETH"), ("SOL-USDT", "SOL"),
    ("XRP-USDT", "XRP"), ("BNB-USDT", "BNB"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[okx] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[okx] connected");
                let (mut write, mut read) = ws_stream.split();

                let args: Vec<_> = OKX_PAIRS.iter().map(|(inst, _)| json!({"channel":"tickers","instId":inst})).collect();
                let sub = json!({"op":"subscribe","args":args});
                if let Err(e) = write.send(txt(sub.to_string())).await { error!("[okx] subscribe: {e}"); continue; }
                info!("[okx] subscribed {} pairs", OKX_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(t)) => {
                            let text = t.to_string();
                            if text == "ping" { let _ = write.send(txt("pong".to_string())).await; continue; }
                            match parse_message(&text) {
                                Ok(Some(p)) => {
                                    let e = last.entry(p.symbol.clone()).or_insert((0.0, 0.0));
                                    if p.bid == e.0 && p.ask == e.1 { continue; }
                                    *e = (p.bid, p.ask);
                                    if tx.send(p).await.is_err() { return; }
                                }
                                Ok(None) => {}
                                Err(e) => warn!("[okx] parse: {e}"),
                            }
                        }
                        Ok(_) => {}
                        Err(e) => { warn!("[okx] ws: {e}"); break; }
                    }
                }
                warn!("[okx] disconnected, reconnecting...");
            }
            Err(e) => error!("[okx] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("event").is_some() { return Ok(None); }
    let inst_id = v.get("arg").and_then(|a| a.get("instId")).and_then(|i| i.as_str()).unwrap_or("");
    let symbol = OKX_PAIRS.iter().find(|(k, _)| *k == inst_id).map(|(_, s)| *s);
    let symbol = match symbol { Some(s) => s, None => return Ok(None) };
    let data = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()).ok_or_else(|| anyhow::anyhow!("no data"))?;
    let bid: f64 = data.get("bidPx").and_then(|b| b.as_str()).ok_or_else(|| anyhow::anyhow!("no bidPx"))?.parse()?;
    let ask: f64 = data.get("askPx").and_then(|a| a.as_str()).ok_or_else(|| anyhow::anyhow!("no askPx"))?.parse()?;
    let bid_size: f64 = data.get("bidSz").and_then(|b| b.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
    let ask_size: f64 = data.get("askSz").and_then(|a| a.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
    let ts: u64 = data.get("ts").and_then(|t| t.as_str()).unwrap_or("0").parse().unwrap_or_else(|_| now_ms());
    Ok(Some(MarketPrice { exchange: "okx".to_string(), symbol: symbol.to_string(), bid, ask, bid_size, ask_size, timestamp_ms: ts, is_usdt: true }))
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
