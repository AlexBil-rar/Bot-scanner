use anyhow::{anyhow, Result};
use tracing::{info, warn};
use crate::execution::order::{Order, OrderStatus};

pub struct CoinbaseExecutor {
    api_key: String,
    api_secret: String,
    dry_run: bool,
    client: reqwest::Client,
}

impl CoinbaseExecutor {
    pub fn new(api_key: String, api_secret: String, dry_run: bool) -> Self {
        info!("[coinbase-exec] initialized dry_run={}", dry_run);
        Self {
            api_key,
            api_secret,
            dry_run,
            client: reqwest::Client::new(),
        }
    }

    pub async fn execute(&self, order: &mut Order) -> Result<()> {
        if self.dry_run {
            info!("[coinbase-exec] DRY RUN: {}", order);
            // Симулируем исполнение — заполняем по текущей цене
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            order.status = OrderStatus::Filled {
                fill_price: order.price,
                fill_qty: order.quantity,
            };
            order.filled_at_ms = Some(now_ms());
            return Ok(());
        }

        // Реальное исполнение — Coinbase Advanced Trade API
        let symbol = format!("{}-USD", order.symbol);
        let side = match order.side {
            crate::execution::order::OrderSide::Buy => "BUY",
            crate::execution::order::OrderSide::Sell => "SELL",
        };

        let body = serde_json::json!({
            "client_order_id": order.client_order_id,
            "product_id": symbol,
            "side": side,
            "order_configuration": {
                "market_market_ioc": {
                    "base_size": format!("{:.8}", order.quantity)
                }
            }
        });

        let timestamp = now_ms() / 1000;
        let path = "/api/v3/brokerage/orders";
        let signature = self.sign(timestamp, "POST", path, &body.to_string())?;

        let response = self.client
            .post(format!("https://api.coinbase.com{}", path))
            .header("CB-ACCESS-KEY", &self.api_key)
            .header("CB-ACCESS-SIGN", signature)
            .header("CB-ACCESS-TIMESTAMP", timestamp.to_string())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            warn!("[coinbase-exec] order failed: {} {}", status, text);
            order.status = OrderStatus::Rejected { reason: text };
            return Err(anyhow!("order rejected"));
        }

        let resp: serde_json::Value = serde_json::from_str(&text)?;
        let order_id = resp["order_id"].as_str().unwrap_or("unknown");
        info!("[coinbase-exec] order placed: {}", order_id);

        // Ждём исполнения (market order обычно исполняется мгновенно)
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        order.status = OrderStatus::Filled {
            fill_price: order.price,
            fill_qty: order.quantity,
        };
        order.filled_at_ms = Some(now_ms());

        Ok(())
    }

    fn sign(&self, timestamp: u64, method: &str, path: &str, body: &str) -> Result<String> {
        use std::fmt::Write;

        let message = format!("{}{}{}{}", timestamp, method, path, body);

        // HMAC-SHA256
        let secret_bytes = hex::decode(&self.api_secret)
            .map_err(|_| anyhow!("invalid api secret (not hex)"))?;

        let mut mac = hmac_sha256::HMAC::new(&secret_bytes);
        mac.update(message.as_bytes());
        let result = mac.finalize();

        let mut hex_str = String::new();
        for byte in result {
            write!(hex_str, "{:02x}", byte).unwrap();
        }
        Ok(hex_str)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
