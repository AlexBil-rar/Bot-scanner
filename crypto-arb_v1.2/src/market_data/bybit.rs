use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";
fn txt(s: String) -> Message { Message::Text(s.into()) }

const BYBIT_PAIRS: &[(&str, &str)] = &[
    ("BTCUSDT", "BTC"), ("ETHUSDT", "ETH"), ("SOLUSDT", "SOL"),
    ("XRPUSDT", "XRP"), ("BNBUSDT", "BNB"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[bybit] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[bybit] connected");
                let (mut write, mut read) = ws_stream.split();

                let args: Vec<String> = BYBIT_PAIRS.iter().map(|(sym, _)| format!("orderbook.1.{}", sym)).collect();
                let sub = json!({"op":"subscribe","args":args});
                if let Err(e) = write.send(txt(sub.to_string())).await { error!("[bybit] subscribe: {e}"); continue; }
                info!("[bybit] subscribed {} pairs", BYBIT_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                let mut ping = tokio::time::interval(tokio::time::Duration::from_secs(20));

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(t))) => {
                                    match parse_message(&t.to_string()) {
                                        Ok(Some(p)) => {
                                            let e = last.entry(p.symbol.clone()).or_insert((0.0, 0.0));
                                            if p.bid == e.0 && p.ask == e.1 { continue; }
                                            *e = (p.bid, p.ask);
                                            if tx.send(p).await.is_err() { return; }
                                        }
                                        Ok(None) => {}
                                        Err(e) => warn!("[bybit] parse: {e}"),
                                    }
                                }
                                Some(Ok(_)) => {}
                                Some(Err(e)) => { warn!("[bybit] ws: {e}"); break; }
                                None => break,
                            }
                        }
                        _ = ping.tick() => { let _ = write.send(txt(json!({"op":"ping"}).to_string())).await; }
                    }
                }
                warn!("[bybit] disconnected, reconnecting...");
            }
            Err(e) => error!("[bybit] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("op").is_some() { return Ok(None); }
    let topic = v.get("topic").and_then(|t| t.as_str()).unwrap_or("");
    let symbol = BYBIT_PAIRS.iter().find(|(sym, _)| topic.contains(*sym)).map(|(_, s)| *s);
    let symbol = match symbol { Some(s) => s, None => return Ok(None) };
    let data = v.get("data").ok_or_else(|| anyhow::anyhow!("no data"))?;
    let bid_arr = data.get("b").and_then(|b| b.as_array()).and_then(|b| b.first()).and_then(|b| b.as_array());
    let ask_arr = data.get("a").and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|a| a.as_array());
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
    let ts = v.get("ts").and_then(|t| t.as_u64()).unwrap_or_else(now_ms);
    Ok(Some(MarketPrice { exchange: "bybit".to_string(), symbol: symbol.to_string(), bid, ask, bid_size, ask_size, timestamp_ms: ts, is_usdt: true }))
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
