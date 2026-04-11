use anyhow::{anyhow, Result};
use tracing::{info, warn};
use crate::execution::executor::{Order, OrderStatus, OrderSide};

pub struct BybitExecutor {
    api_key: String,
    api_secret: String,
    dry_run: bool,
    client: reqwest::Client,
}

impl BybitExecutor {
    pub fn new(api_key: String, api_secret: String, dry_run: bool) -> Self {
        info!("[bybit-exec] initialized dry_run={}", dry_run);
        Self { api_key, api_secret, dry_run, client: reqwest::Client::new() }
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

        let symbol = format!("{}USDT", order.symbol);
        let side = match order.side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        };

        let timestamp = now_ms().to_string();
        let body = serde_json::json!({
            "category": "spot",
            "symbol": symbol,
            "side": side,
            "orderType": "Market",
            "qty": format!("{:.8}", order.quantity),
        });
        let body_str = body.to_string();

        let recv_window = "5000";
        let sign_str = format!("{}{}{}{}", timestamp, &self.api_key, recv_window, body_str);
        let signature = self.hmac_sign(&sign_str)?;

        let response = self.client
            .post("https://api.bybit.com/v5/order/create")
            .header("X-BAPI-API-KEY", &self.api_key)
            .header("X-BAPI-SIGN", &signature)
            .header("X-BAPI-TIMESTAMP", &timestamp)
            .header("X-BAPI-RECV-WINDOW", recv_window)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;

        let text = response.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&text)?;

        let ret_code = resp["retCode"].as_i64().unwrap_or(-1);
        if ret_code != 0 {
            let reason = resp["retMsg"].as_str().unwrap_or(&text).to_string();
            warn!("[bybit-exec] order failed: {}", reason);
            order.status = OrderStatus::Rejected { reason: reason.clone() };
            return Err(anyhow!("order rejected: {}", reason));
        }

        let order_id = resp["result"]["orderId"].as_str().unwrap_or("?");
        info!("[bybit-exec] order placed: {}", order_id);
        order.status = OrderStatus::Filled {
            fill_price: order.price,
            fill_qty: order.quantity,
        };
        order.filled_at_ms = Some(now_ms());
        Ok(())
    }

    /// Market buy по сумме в USDT (для inventory optimizer)
    /// Bybit V5 spot market buy: qty = quote amount when marketUnit is not set and side=Buy
    pub async fn buy_by_quote(&self, symbol: &str, usdt_amount: f64) -> Result<()> {
        if self.dry_run { return Ok(()); }

        let pair = format!("{}USDT", symbol);
        let timestamp = now_ms().to_string();
        let body = serde_json::json!({
            "category": "spot",
            "symbol": pair,
            "side": "Buy",
            "orderType": "Market",
            "qty": format!("{:.2}", usdt_amount),
            "marketUnit": "quoteCoin",
        });
        let body_str = body.to_string();

        let recv_window = "5000";
        let sign_str = format!("{}{}{}{}", timestamp, &self.api_key, recv_window, body_str);
        let signature = self.hmac_sign(&sign_str)?;

        let response = self.client
            .post("https://api.bybit.com/v5/order/create")
            .header("X-BAPI-API-KEY", &self.api_key)
            .header("X-BAPI-SIGN", &signature)
            .header("X-BAPI-TIMESTAMP", &timestamp)
            .header("X-BAPI-RECV-WINDOW", recv_window)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;

        let text = response.text().await?;
        let resp: serde_json::Value = serde_json::from_str(&text)?;

        let ret_code = resp["retCode"].as_i64().unwrap_or(-1);
        if ret_code != 0 {
            let reason = resp["retMsg"].as_str().unwrap_or(&text).to_string();
            warn!("[bybit-exec] buy_by_quote failed: {}", reason);
            return Err(anyhow!("order rejected: {}", reason));
        }

        info!("[bybit-exec] buy_by_quote OK: {} ${:.2} USDT", symbol, usdt_amount);
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

    /// Получить ВСЕ балансы: крипта + стейблкоины (Unified Account)
    pub async fn get_all_balances(&self) -> Vec<(String, f64)> {
        if self.dry_run {
            return vec![
                ("USDT".to_string(), 50.0),
                ("XRP".to_string(), 38.0),
                ("SOL".to_string(), 0.6),
            ];
        }

        info!("[bybit-bal] fetching balances...");

        let timestamp = now_ms().to_string();
        let recv_window = "5000";
        let params = "accountType=UNIFIED";
        let sign_str = format!("{}{}{}{}", timestamp, &self.api_key, recv_window, params);
        let signature = match self.hmac_sign(&sign_str) {
            Ok(s) => s,
            Err(e) => { warn!("[bybit-bal] HMAC sign failed: {}", e); return vec![]; }
        };

        info!("[bybit-bal] sending request...");

        let resp = self.client
            .get(format!("https://api.bybit.com/v5/account/wallet-balance?{}", params))
            .header("X-BAPI-API-KEY", &self.api_key)
            .header("X-BAPI-SIGN", &signature)
            .header("X-BAPI-TIMESTAMP", &timestamp)
            .header("X-BAPI-RECV-WINDOW", recv_window)
            .send().await;

        let resp = match resp {
            Ok(r) => { info!("[bybit-bal] got response status={}", r.status()); r }
            Err(e) => { warn!("[bybit-bal] request FAILED: {}", e); return vec![]; }
        };
        let text = match resp.text().await {
            Ok(t) => { info!("[bybit-bal] body length={}", t.len()); t }
            Err(e) => { warn!("[bybit-bal] read body failed: {}", e); return vec![]; }
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => { warn!("[bybit-bal] JSON parse failed: {} | raw: {}", e, &text[..text.len().min(300)]); return vec![]; }
        };

        let ret_code = v["retCode"].as_i64().unwrap_or(-1);
        if ret_code != 0 {
            warn!("[bybit-bal] API error: retCode={} retMsg={}", ret_code, v["retMsg"]);
            return vec![];
        }

        let mut result = vec![];

        if let Some(list) = v["result"]["list"].as_array() {
            for account in list {
                if let Some(coins) = account["coin"].as_array() {
                    for coin in coins {
                        if let (Some(name), Some(equity)) = (coin["coin"].as_str(), coin["walletBalance"].as_str()) {
                            if let Ok(amount) = equity.parse::<f64>() {
                                if amount > 0.0001 {
                                    info!("[bybit-bal] {} = {:.4}", name, amount);
                                    result.push((name.to_string(), amount));
                                }
                            }
                        }
                    }
                }
            }
        } else {
            warn!("[bybit-bal] no result.list in response: {}", &text[..text.len().min(300)]);
        }

        if result.is_empty() {
            warn!("[bybit-bal] no balances found after parsing");
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