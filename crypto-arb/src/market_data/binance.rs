use anyhow::Result;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use crate::models::MarketPrice;

const WS_URL: &str = "wss://stream.binance.com:9443/stream?streams=\
    btcusdt@bookTicker/ethusdt@bookTicker/solusdt@bookTicker/\
    xrpusdt@bookTicker/bnbusdt@bookTicker";

#[derive(Debug, Deserialize)]
struct StreamWrapper { stream: String, data: BinanceBookTicker }

#[derive(Debug, Deserialize)]
struct BinanceBookTicker {
    #[serde(rename = "b")] bid: String,
    #[serde(rename = "B")] bid_qty: String,
    #[serde(rename = "a")] ask: String,
    #[serde(rename = "A")] ask_qty: String,
    #[serde(rename = "T")] transaction_time: Option<u64>,
}

pub async fn stream(tx: mpsc::Sender<MarketPrice>) {
    loop {
        info!("[binance] connecting (BTC+ETH+SOL+XRP+BNB)...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[binance] connected");
                let (_, mut read) = ws_stream.split();
                let mut last: std::collections::HashMap<String, (f64, f64)> = Default::default();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(t)) => {
                            match parse_message(&t.to_string()) {
                                Ok(price) => {
                                    let e = last.entry(price.symbol.clone()).or_insert((0.0, 0.0));
                                    if price.bid == e.0 && price.ask == e.1 { continue; }
                                    *e = (price.bid, price.ask);
                                    if tx.send(price).await.is_err() { return; }
                                }
                                Err(e) => warn!("[binance] parse: {e}"),
                            }
                        }
                        Ok(_) => {}
                        Err(e) => { warn!("[binance] ws error: {e}"); break; }
                    }
                }
                warn!("[binance] disconnected, reconnecting...");
            }
            Err(e) => error!("[binance] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

fn stream_to_symbol(stream: &str) -> &'static str {
    if stream.contains("btc")  { "BTC" }
    else if stream.contains("eth")  { "ETH" }
    else if stream.contains("sol")  { "SOL" }
    else if stream.contains("xrp")  { "XRP" }
    else if stream.contains("bnb")  { "BNB" }
    else { "UNKNOWN" }
}

fn parse_message(text: &str) -> Result<MarketPrice> {
    let w: StreamWrapper = serde_json::from_str(text)?;
    let symbol = stream_to_symbol(&w.stream);
    Ok(MarketPrice {
        exchange: "binance".to_string(),
        symbol: symbol.to_string(),
        bid: w.data.bid.parse()?,
        ask: w.data.ask.parse()?,
        bid_size: w.data.bid_qty.parse().unwrap_or(0.0),
        ask_size: w.data.ask_qty.parse().unwrap_or(0.0),
        timestamp_ms: w.data.transaction_time.unwrap_or_else(now_ms),
        is_usdt: true,
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
