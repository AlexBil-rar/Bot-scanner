use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://api.gateio.ws/ws/v4/";
fn txt(s: String) -> Message { Message::Text(s.into()) }

const GATEIO_PAIRS: &[(&str, &str)] = &[
    ("BTC_USDT", "BTC"), ("ETH_USDT", "ETH"), ("SOL_USDT", "SOL"),
    ("XRP_USDT", "XRP"), ("BNB_USDT", "BNB"),
];

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[gateio] connecting...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[gateio] connected");
                let (mut write, mut read) = ws_stream.split();

                for (pair, _) in GATEIO_PAIRS {
                    let sub = json!({"time": now_ms()/1000, "channel": "spot.book_ticker", "event": "subscribe", "payload": [pair]});
                    if let Err(e) = write.send(txt(sub.to_string())).await { error!("[gateio] subscribe {pair}: {e}"); }
                }
                info!("[gateio] subscribed {} pairs", GATEIO_PAIRS.len());

                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();
                let mut ping = tokio::time::interval(tokio::time::Duration::from_secs(25));

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
                                        Err(e) => warn!("[gateio] parse: {e}"),
                                    }
                                }
                                Some(Ok(_)) => {}
                                Some(Err(e)) => { warn!("[gateio] ws: {e}"); break; }
                                None => break,
                            }
                        }
                        _ = ping.tick() => { let _ = write.send(txt(json!({"time": now_ms()/1000, "channel": "spot.ping"}).to_string())).await; }
                    }
                }
                warn!("[gateio] disconnected, reconnecting...");
            }
            Err(e) => error!("[gateio] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn parse_message(text: &str) -> Result<Option<MarketPrice>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    if v.get("event").and_then(|e| e.as_str()) != Some("update") { return Ok(None); }
    if v.get("channel").and_then(|c| c.as_str()) != Some("spot.book_ticker") { return Ok(None); }
    let r = v.get("result").ok_or_else(|| anyhow::anyhow!("no result"))?;
    let pair = r.get("s").and_then(|p| p.as_str()).unwrap_or("");
    let symbol = GATEIO_PAIRS.iter().find(|(k, _)| *k == pair).map(|(_, s)| *s);
    let symbol = match symbol { Some(s) => s, None => return Ok(None) };
    let bid: f64 = r.get("b").and_then(|b| b.as_str()).ok_or_else(|| anyhow::anyhow!("no b"))?.parse()?;
    let ask: f64 = r.get("a").and_then(|a| a.as_str()).ok_or_else(|| anyhow::anyhow!("no a"))?.parse()?;
    let bid_size: f64 = r.get("B").and_then(|b| b.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
    let ask_size: f64 = r.get("A").and_then(|a| a.as_str()).unwrap_or("0").parse().unwrap_or(0.0);
    Ok(Some(MarketPrice { exchange: "gateio".to_string(), symbol: symbol.to_string(), bid, ask, bid_size, ask_size, timestamp_ms: now_ms(), is_usdt: true }))
}

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 }
