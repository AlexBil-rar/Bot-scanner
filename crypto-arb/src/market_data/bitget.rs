use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://ws.bitget.com/v2/ws/public";
fn txt(s: String) -> Message { Message::Text(s.into()) }

const BITGET_PAIRS: &[(&str, &str)] = &[
    ("BTCUSDT", "BTC"), ("ETHUSDT", "ETH"), ("SOLUSDT", "SOL"),
    ("XRPUSDT", "XRP"), ("BNBUSDT", "BNB"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[bitget] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[bitget] connected");
                let (mut write, mut read) = ws_stream.split();

                let args: Vec<_> = BITGET_PAIRS.iter().map(|(inst, _)| json!({"instType":"SPOT","channel":"books1","instId":inst})).collect();
                let sub = json!({"op":"subscribe","args":args});
                if let Err(e) = write.send(txt(sub.to_string())).await { error!("[bitget] subscribe: {e}"); continue; }
                info!("[bitget] subscribed {} pairs", BITGET_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                let mut ping = tokio::time::interval(tokio::time::Duration::from_secs(25));

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(t))) => {
                                    let text = t.to_string();
                                    if text.trim() == "ping" { let _ = write.send(txt("pong".to_string())).await; continue; }
                                    match parse_message(&text) {
                                        Ok(Some(p)) => {
                                            let e = last.entry(p.symbol.clone()).or_insert((0.0, 0.0));
                                            if p.bid == e.0 && p.ask == e.1 { continue; }
                                            *e = (p.bid, p.ask);
                                            if tx.send(p).await.is_err() { return; }
                                        }
                                        Ok(None) => {}
                                        Err(e) => warn!("[bitget] parse: {e}"),
                                    }
                                }
                                Some(Ok(Message::Binary(_))) => { let _ = write.send(txt("pong".to_string())).await; }
                                Some(Ok(_)) => {}
                                Some(Err(e)) => { warn!("[bitget] ws: {e}"); break; }
                                None => break,
                            }
                        }
                        _ = ping.tick() => { let _ = write.send(txt("ping".to_string())).await; }
                    }
                }
                warn!("[bitget] disconnected, reconnecting...");
            }
            Err(e) => error!("[bitget] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("event").is_some() { return Ok(None); }
    let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("");
    if action != "snapshot" && action != "update" { return Ok(None); }
    let inst_id = v.get("arg").and_then(|a| a.get("instId")).and_then(|i| i.as_str()).unwrap_or("");
    let symbol = BITGET_PAIRS.iter().find(|(k, _)| *k == inst_id).map(|(_, s)| *s);
    let symbol = match symbol { Some(s) => s, None => return Ok(None) };
    let data = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()).ok_or_else(|| anyhow::anyhow!("no data"))?;
    let bid_arr = data.get("bids").and_then(|b| b.as_array()).and_then(|b| b.first()).and_then(|b| b.as_array());
    let ask_arr = data.get("asks").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|a| a.as_array());
    let (bid, bid_size) = match bid_arr {
        Some(a) => (a[0].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                    a.get(1).and_then(|v| v.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0)),
        None => return Ok(None),
    };
    let (ask, ask_size) = match ask_arr {
        Some(a) => (a[0].as_str().unwrap_or("0").parse::<f64>().unwrap_or(0.0),
                    a.get(1).and_then(|v| v.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0)),
        None => return Ok(None),
    };
    let ts: u64 = data.get("ts").and_then(|t| t.as_str()).unwrap_or("0").parse().unwrap_or_else(|_| now_ms());
    Ok(Some(MarketPrice { exchange: "bitget".to_string(), symbol: symbol.to_string(), bid, ask, bid_size, ask_size, timestamp_ms: ts, is_usdt: true }))
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
