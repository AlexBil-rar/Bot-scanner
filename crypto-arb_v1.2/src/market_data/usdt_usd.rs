//! Отслеживаем текущий курс USDT/USD
//! Источник: Binance USDCUSDT (USDC ≈ USD) + Kraken USD/USDT
//! Храним в AtomicU64 для lock-free чтения из детектора

use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Глобальный курс USDT/USD (хранится как u64 = price * 1_000_000)
pub type UsdtRate = Arc<AtomicU64>;

pub fn new_rate() -> UsdtRate {
    // По умолчанию 1.0000 (1_000_000 в fixed point)
    Arc::new(AtomicU64::new(1_000_000))
}

pub fn get_rate(rate: &UsdtRate) -> f64 {
    rate.load(Ordering::Relaxed) as f64 / 1_000_000.0
}

pub fn set_rate(rate: &UsdtRate, value: f64) {
    rate.store((value * 1_000_000.0) as u64, Ordering::Relaxed);
}

const WS_URL: &str = "wss://stream.binance.com:9443/ws/usdcusdt@bookTicker";

pub async fn stream(rate: UsdtRate) {
    loop {
        info!("[usdt-usd] connecting (USDC/USDT ≈ USD/USDT)...");
        match connect_async(WS_URL).await {
            Ok((ws_stream, _)) => {
                info!("[usdt-usd] connected");
                let (_, mut read) = ws_stream.split();
                while let Some(msg) = read.next().await {
                    if let Ok(Message::Text(t)) = msg {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t.to_string()) {
                            // bid = сколько USDT стоит 1 USDC
                            // USDC ≈ USD, поэтому bid ≈ 1/USDT_price
                            // Если USDC/USDT bid=0.9998 → USDT = 1/0.9998 = 1.0002 USD
                            if let Some(bid_str) = v.get("b").and_then(|b| b.as_str()) {
                                if let Ok(usdc_per_usdt) = bid_str.parse::<f64>() {
                                    if usdc_per_usdt > 0.0 {
                                        let usdt_in_usd = 1.0 / usdc_per_usdt;
                                        set_rate(&rate, usdt_in_usd);
                                        // Логируем только если отклоняется > 0.5%
                                        if (usdt_in_usd - 1.0).abs() > 0.005 {
                                            warn!("[usdt-usd] USDT = ${:.6} (deviation={:+.4}%)",
                                                usdt_in_usd, (usdt_in_usd - 1.0) * 100.0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                warn!("[usdt-usd] disconnected, reconnecting...");
            }
            Err(e) => error!("[usdt-usd] failed: {e}"),
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
