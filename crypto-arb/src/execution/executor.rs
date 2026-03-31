use tracing::{error, info, warn};

use crate::execution::bybit::BybitExecutor;
use crate::execution::coinbase::CoinbaseExecutor;
use crate::execution::order::{ArbitrageExecution, Order};
use crate::models::ArbitrageOpportunity;

/// Конфиг для execution engine
pub struct ExecutionConfig {
    /// true = только логировать, не отправлять реальные ордера
    pub dry_run: bool,
    /// Минимальный score для исполнения
    pub min_score: u8,
    /// Максимальный размер одной сделки в USD
    pub max_trade_usd: f64,
    /// Не исполнять stale сигналы
    pub skip_stale: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            dry_run: true,       // БЕЗОПАСНО по умолчанию
            min_score: 6,        // только strong
            max_trade_usd: 500.0,
            skip_stale: true,
        }
    }
}

pub struct ArbitrageExecutor {
    config: ExecutionConfig,
    coinbase: CoinbaseExecutor,
    bybit: BybitExecutor,
    total_executions: u64,
    total_profit_usd: f64,
}

impl ArbitrageExecutor {
    pub fn new(
        config: ExecutionConfig,
        coinbase_key: String, coinbase_secret: String,
        bybit_key: String, bybit_secret: String,
    ) -> Self {
        let dry = config.dry_run;
        Self {
            coinbase: CoinbaseExecutor::new(coinbase_key, coinbase_secret, dry),
            bybit: BybitExecutor::new(bybit_key, bybit_secret, dry),
            config,
            total_executions: 0,
            total_profit_usd: 0.0,
        }
    }

    pub async fn try_execute(&mut self, opp: &ArbitrageOpportunity) -> Option<ArbitrageExecution> {
        // Фильтры перед исполнением
        if opp.signal_score < self.config.min_score {
            return None;
        }
        if self.config.skip_stale && opp.is_stale {
            return None;
        }
        // Поддерживаем только coinbase→bybit для начала
        if opp.buy_exchange != "coinbase" || opp.sell_exchange != "bybit" {
            return None;
        }

        let trade_usd = opp.max_size_usd.min(self.config.max_trade_usd);
        let quantity = trade_usd / opp.buy_price;

        info!(
            "⚡ [EXEC] {} {}→{} profit={:.4}% size=${:.0} qty={:.6} score={}/10 {}",
            opp.symbol, opp.buy_exchange, opp.sell_exchange,
            opp.estimated_profit_pct, trade_usd, quantity,
            opp.signal_score,
            if self.config.dry_run { "[DRY RUN]" } else { "[LIVE]" }
        );

        let signal_ts = opp.timestamp_ms;

        let mut buy_order = Order::market_buy(&opp.buy_exchange, &opp.symbol, quantity);
        buy_order.price = opp.buy_price;

        let mut sell_order = Order::market_sell(&opp.sell_exchange, &opp.symbol, quantity);
        sell_order.price = opp.sell_price;

        // Отправляем оба ордера параллельно
        let (buy_result, sell_result) = tokio::join!(
            self.coinbase.execute(&mut buy_order),
            self.bybit.execute(&mut sell_order),
        );

        let latency_ms = now_ms() - signal_ts;
        let success = buy_result.is_ok() && sell_result.is_ok();

        if let Err(e) = &buy_result  { error!("[EXEC] buy failed: {e}"); }
        if let Err(e) = &sell_result { error!("[EXEC] sell failed: {e}"); }

        // Считаем реальную прибыль
        let actual_profit_usd = if success {
            let buy_cost  = buy_order.quantity  * opp.buy_price  * (1.0 + 0.0005); // +fee
            let sell_recv = sell_order.quantity * opp.sell_price * (1.0 - 0.0010); // -fee
            sell_recv - buy_cost
        } else {
            0.0
        };

        if success {
            self.total_executions += 1;
            self.total_profit_usd += actual_profit_usd;
            info!(
                "✅ [EXEC] done | profit=${:.4} | latency={}ms | total_profit=${:.4}",
                actual_profit_usd, latency_ms, self.total_profit_usd
            );
        } else {
            warn!("[EXEC] execution failed — skipping");
        }

        Some(ArbitrageExecution {
            buy_order,
            sell_order,
            signal_profit_pct: opp.estimated_profit_pct,
            actual_profit_usd,
            latency_ms,
            success,
        })
    }

    pub fn report(&self) {
        info!(
            "[exec] total_executions={} total_profit=${:.4}",
            self.total_executions, self.total_profit_usd
        );
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
