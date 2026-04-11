use anyhow::Result;
use tracing::{info, warn};
use crate::execution::executor::{Order, OrderStatus, OrderSide};

pub struct BinanceExecutor {
    api_key: String,
    api_secret: String,
    dry_run: bool,
    client: reqwest::Client,
}

impl BinanceExecutor {
    pub fn new(api_key: String, api_secret: String, dry_run: bool) -> Self {
        info!("[binance-exec] initialized dry_run={}", dry_run);
        Self { api_key, api_secret, dry_run, client: reqwest::Client::new() }
    }

    pub async fn execute(&self, order: &mut Order) -> Result<()> {
        if self.dry_run {
            info!("[binance-exec] DRY RUN: {}", order);
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            order.status = OrderStatus::Filled {
                fill_price: order.price,
                fill_qty: order.quantity,
            };
            order.filled_at_ms = Some(now_ms());
            return Ok(());
        }

        let symbol = format!("{}USDT", order.symbol);
        let side = match order.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let timestamp = now_ms();
        let params = format!(
            "symbol={}&side={}&type=MARKET&quantity={:.8}&timestamp={}&recvWindow=5000",
            symbol, side, order.quantity, timestamp
        );
        let signature = self.hmac_sign(&params)?;
        let body = format!("{}&signature={}", params, signature);

        let response = self.client
            .post("https://api.binance.com/api/v3/order")
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;

        let text = response.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&text)?;

        if resp["code"].is_number() {
            let reason = resp["msg"].as_str().unwrap_or(&text).to_string();
            warn!("[binance-exec] order failed: {}", reason);
            order.status = OrderStatus::Rejected { reason };
            return Err(anyhow::anyhow!("order rejected"));
        }

        info!("[binance-exec] order placed: {}", resp["orderId"]);
        order.status = OrderStatus::Filled {
            fill_price: order.price,
            fill_qty: order.quantity,
        };
        order.filled_at_ms = Some(now_ms());
        Ok(())
    }

    /// Market buy по сумме в USDT (для inventory optimizer)
    pub async fn buy_by_quote(&self, symbol: &str, usdt_amount: f64) -> Result<()> {
        if self.dry_run { return Ok(()); }

        let pair = format!("{}USDT", symbol);
        let timestamp = now_ms();
        let params = format!(
            "symbol={}&side=BUY&type=MARKET&quoteOrderQty={:.2}&timestamp={}&recvWindow=5000",
            pair, usdt_amount, timestamp
        );
        let signature = self.hmac_sign(&params)?;
        let body = format!("{}&signature={}", params, signature);

        let response = self.client
            .post("https://api.binance.com/api/v3/order")
            .header("X-MBX-APIKEY", &self.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await?;

        let text = response.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&text)?;

        if resp["code"].is_number() {
            let reason = resp["msg"].as_str().unwrap_or(&text).to_string();
            warn!("[binance-exec] buy_by_quote failed: {}", reason);
            return Err(anyhow::anyhow!("order rejected: {}", reason));
        }

        info!("[binance-exec] buy_by_quote OK: {} ${:.2} USDT", symbol, usdt_amount);
        Ok(())
    }

    pub async fn get_balance_usdt(&self) -> f64 {
        if self.dry_run { return 10000.0; }
        let balances = self.get_all_balances().await;
        balances.iter()
            .filter(|(asset, _)| *asset == "USDT")
            .map(|(_, amt)| amt)
            .sum()
    }

    /// Получить ВСЕ балансы: крипта + стейблкоины
    pub async fn get_all_balances(&self) -> Vec<(String, f64)> {
        if self.dry_run {
            return vec![
                ("USDT".to_string(), 50.0),
                ("XRP".to_string(), 38.0),
                ("SOL".to_string(), 0.6),
            ];
        }

        let timestamp = now_ms();
        let params = format!("timestamp={}&recvWindow=5000", timestamp);
        let signature = match self.hmac_sign(&params) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let resp = self.client
            .get(format!("https://api.binance.com/api/v3/account?{}&signature={}", params, signature))
            .header("X-MBX-APIKEY", &self.api_key)
            .send().await;

        let resp = match resp { Ok(r) => r, Err(_) => return vec![] };
        let text = match resp.text().await { Ok(t) => t, Err(_) => return vec![] };
        let v: serde_json::Value = match serde_json::from_str(&text) { Ok(v) => v, Err(_) => return vec![] };

        let mut result = vec![];
        if let Some(balances) = v["balances"].as_array() {
            for bal in balances {
                if let (Some(asset), Some(free)) = (bal["asset"].as_str(), bal["free"].as_str()) {
                    if let Ok(amount) = free.parse::<f64>() {
                        if amount > 0.0001 {
                            info!("[binance-bal] {} = {:.4}", asset, amount);
                            result.push((asset.to_string(), amount));
                        }
                    }
                }
            }
        }
        result
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
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
