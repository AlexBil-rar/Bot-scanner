use anyhow::{anyhow, Result};
use tracing::{info, warn};
use crate::execution::executor::{Order, OrderStatus, OrderSide};

pub struct CoinbaseExecutor {
    api_key: String,
    api_secret: String,
    dry_run: bool,
    client: reqwest::Client,
}

impl CoinbaseExecutor {
    pub fn new(api_key: String, api_secret: String, dry_run: bool) -> Self {
        info!("[coinbase-exec] initialized dry_run={}", dry_run);
        Self { api_key, api_secret, dry_run, client: reqwest::Client::new() }
    }

    pub async fn execute(&self, order: &mut Order) -> Result<()> {
        if self.dry_run {
            info!("[coinbase-exec] DRY RUN: {}", order);
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            order.status = OrderStatus::Filled {
                fill_price: order.price,
                fill_qty: order.quantity,
            };
            order.filled_at_ms = Some(now_ms());
            return Ok(());
        }

        let side = match order.side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        let symbol = format!("{}-USDC", order.symbol);
        let qty = format!("{:.4}", order.quantity);
        let client_order_id = &order.client_order_id;
        let api_key = &self.api_key;
        let pem_path = &self.api_secret;

        let script = format!(r#"
import jwt, time, secrets, urllib.request, json
from cryptography.hazmat.primitives.serialization import load_pem_private_key
from pathlib import Path

api_key = '{api_key}'
pem = Path('{pem_path}').read_bytes()
key = load_pem_private_key(pem, password=None)

ts = int(time.time())
path = '/api/v3/brokerage/orders'
payload = {{'sub': api_key, 'iss': 'cdp', 'nbf': ts, 'exp': ts+120, 'uri': f'POST api.coinbase.com{{path}}'}}
token = jwt.encode(payload, key, algorithm='ES256', headers={{'kid': api_key, 'nonce': secrets.token_hex(16)}})

body = json.dumps({{
    'client_order_id': '{client_order_id}',
    'product_id': '{symbol}',
    'side': '{side}',
    'order_configuration': {{
        'market_market_ioc': {{
            'base_size': '{qty}'
        }}
    }}
}}).encode()

req = urllib.request.Request(f'https://api.coinbase.com{{path}}',
    data=body,
    headers={{'Authorization': f'Bearer {{token}}', 'Content-Type': 'application/json'}},
    method='POST')

try:
    with urllib.request.urlopen(req, timeout=5) as r:
        data = json.loads(r.read())
        if data.get('success'):
            print('OK:' + data['success_response']['order_id'])
        else:
            print('FAIL:' + json.dumps(data))
except urllib.error.HTTPError as e:
    print('ERROR:' + str(e.code) + ':' + e.read().decode()[:200])
"#,
            api_key = api_key,
            pem_path = pem_path,
            client_order_id = client_order_id,
            symbol = symbol,
            side = side,
            qty = qty,
        );

        let output = tokio::process::Command::new("python3")
            .arg("-c")
            .arg(&script)
            .output()
            .await?;

        let stdout = String::from_utf8(output.stdout)?.trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !stderr.is_empty() {
            warn!("[coinbase-exec] stderr: {}", stderr.trim());
        }

        if stdout.starts_with("OK:") {
            let order_id = &stdout[3..];
            info!("[coinbase-exec] order placed: {}", order_id);
            order.status = OrderStatus::Filled {
                fill_price: order.price,
                fill_qty: order.quantity,
            };
            order.filled_at_ms = Some(now_ms());
            Ok(())
        } else {
            let reason = stdout.clone();
            warn!("[coinbase-exec] order failed: {}", reason);
            order.status = OrderStatus::Rejected { reason: reason.clone() };
            Err(anyhow!("order rejected: {}", reason))
        }
    }

    pub async fn get_balance_usd(&self) -> f64 {
        if self.dry_run { return 10000.0; }

        let balances = self.get_all_balances().await;
        balances.iter()
            .filter(|(asset, _)| *asset == "USD" || *asset == "USDC")
            .map(|(_, amt)| amt)
            .sum()
    }

    /// Получить ВСЕ балансы: крипта + стейблкоины
    pub async fn get_all_balances(&self) -> Vec<(String, f64)> {
        if self.dry_run {
            return vec![
                ("USDC".to_string(), 50.0),
                ("XRP".to_string(), 38.0),
                ("SOL".to_string(), 0.6),
            ];
        }

        let pem_path = &self.api_secret;
        let api_key = &self.api_key;

        let script = format!(
            "from coinbase.rest import RESTClient\nfrom pathlib import Path\nsecret = Path('{}').read_text() if Path('{}').exists() else '{}'\nclient = RESTClient(api_key='{}', api_secret=secret.strip())\nfor acc in client.get_accounts().accounts:\n    bal = float(acc.available_balance['value'] or 0)\n    if bal > 0.0001:\n        print(f'{{acc.currency}}:{{bal}}')\n",
            pem_path, pem_path, pem_path, api_key
        );

        let output = tokio::process::Command::new("python3")
            .arg("-c")
            .arg(&script)
            .output()
            .await;

        let mut result = vec![];
        if let Ok(out) = output {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.is_empty() {
                warn!("[coinbase-bal] error: {}", stderr.trim());
            }
            if let Ok(s) = String::from_utf8(out.stdout) {
                for line in s.lines() {
                    if let Some((asset, amt_str)) = line.split_once(':') {
                        if let Ok(amt) = amt_str.trim().parse::<f64>() {
                            if amt > 0.0001 {
                                info!("[coinbase-bal] {} = {:.4}", asset, amt);
                                result.push((asset.to_string(), amt));
                            }
                        }
                    }
                }
            }
        }

        if result.is_empty() {
            // fallback
            let capital: f64 = std::env::var("CAPITAL_PER_EXCHANGE")
                .ok().and_then(|v| v.parse().ok()).unwrap_or(50.0);
            result.push(("USDC".to_string(), capital));
        }

        result
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
