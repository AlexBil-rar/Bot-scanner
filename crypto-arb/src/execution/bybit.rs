use anyhow::{anyhow, Result};
use tracing::{info, warn};
use crate::execution::order::{Order, OrderStatus};

pub struct BybitExecutor {
    api_key: String,
    api_secret: String,
    dry_run: bool,
    client: reqwest::Client,
}

impl BybitExecutor {
    pub fn new(api_key: String, api_secret: String, dry_run: bool) -> Self {
        info!("[bybit-exec] initialized dry_run={}", dry_run);
        Self {
            api_key,
            api_secret,
            dry_run,
            client: reqwest::Client::new(),
        }
    }

    pub async fn execute(&self, order: &mut Order) -> Result<()> {
        if self.dry_run {
            info!("[bybit-exec] DRY RUN: {}", order);
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            order.status = OrderStatus::Filled {
                fill_price: order.price,
                fill_qty: order.quantity,
            };
            order.filled_at_ms = Some(now_ms());
            return Ok(());
        }

        // Bybit V5 API — spot market order
        let symbol = format!("{}USDT", order.symbol);
        let side = match order.side {
            crate::execution::order::OrderSide::Buy => "Buy",
            crate::execution::order::OrderSide::Sell => "Sell",
        };

        let timestamp = now_ms();
        let recv_window = 5000u64;

        let body = serde_json::json!({
            "category": "spot",
            "symbol": symbol,
            "side": side,
            "orderType": "Market",
            "qty": format!("{:.8}", order.quantity),
            "orderLinkId": order.client_order_id,
        });

        let body_str = body.to_string();
        let sign_str = format!("{}{}{}{}", timestamp, &self.api_key, recv_window, &body_str);
        let signature = self.hmac_sign(&sign_str)?;

        let response = self.client
            .post("https://api.bybit.com/v5/order/create")
            .header("X-BAPI-API-KEY", &self.api_key)
            .header("X-BAPI-SIGN", signature)
            .header("X-BAPI-SIGN-BY", "2")
            .header("X-BAPI-TIMESTAMP", timestamp.to_string())
            .header("X-BAPI-RECV-WINDOW", recv_window.to_string())
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&text)?;

        if resp["retCode"].as_i64() != Some(0) {
            let reason = resp["retMsg"].as_str().unwrap_or(&text).to_string();
            warn!("[bybit-exec] order failed: {} status={}", reason, status);
            order.status = OrderStatus::Rejected { reason };
            return Err(anyhow!("order rejected"));
        }

        let order_id = resp["result"]["orderId"].as_str().unwrap_or("unknown");
        info!("[bybit-exec] order placed: {}", order_id);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        order.status = OrderStatus::Filled {
            fill_price: order.price,
            fill_qty: order.quantity,
        };
        order.filled_at_ms = Some(now_ms());

        Ok(())
    }

    fn hmac_sign(&self, message: &str) -> Result<String> {
        use std::fmt::Write;
        let mut mac = hmac_sha256::HMAC::new(self.api_secret.as_bytes());
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
