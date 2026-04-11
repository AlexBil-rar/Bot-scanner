use anyhow::Result;
use std::fmt;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::db::Database;
use crate::execution::bitget::BitgetExecutor;
use crate::execution::coinbase::CoinbaseExecutor;
use crate::execution::binance::BinanceExecutor;
use crate::execution::bybit::BybitExecutor;
use crate::execution::inventory::{InventoryManager, LiquidityRecovery};
use crate::models::{ArbitrageOpportunity, TradeResult};

// ===== Order types (inline) =====

#[derive(Debug, Clone, PartialEq)]
pub enum OrderSide { Buy, Sell }

#[derive(Debug, Clone, PartialEq)]
pub enum OrderStatus {
    Pending,
    Filled { fill_price: f64, fill_qty: f64 },
    #[allow(dead_code)]
    PartialFill { fill_price: f64, fill_qty: f64 },
    Rejected { reason: String },
    #[allow(dead_code)]
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub exchange: String,
    pub symbol: String,
    pub side: OrderSide,
    pub quantity: f64,
    pub price: f64,
    pub is_market: bool,
    pub client_order_id: String,
    pub status: OrderStatus,
    pub created_at_ms: u64,
    pub filled_at_ms: Option<u64>,
}

impl Order {
    pub fn market_buy(exchange: &str, symbol: &str, quantity: f64) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            side: OrderSide::Buy,
            quantity, price: 0.0, is_market: true,
            client_order_id: format!("arb-{}", now_ms()),
            status: OrderStatus::Pending,
            created_at_ms: now_ms(),
            filled_at_ms: None,
        }
    }

    pub fn market_sell(exchange: &str, symbol: &str, quantity: f64) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            side: OrderSide::Sell,
            quantity, price: 0.0, is_market: true,
            client_order_id: format!("arb-{}", now_ms()),
            status: OrderStatus::Pending,
            created_at_ms: now_ms(),
            filled_at_ms: None,
        }
    }
}

impl fmt::Display for Order {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?} {} {} @ {}",
            self.exchange, self.side, self.quantity, self.symbol,
            if self.is_market { "MARKET".to_string() } else { format!("{:.4}", self.price) }
        )
    }
}

#[derive(Debug)]
pub struct ArbitrageExecution {
    pub buy_order: Order,
    pub sell_order: Order,
    pub signal_profit_pct: f64,
    pub actual_profit_usd: f64,
    pub latency_ms: u64,
    pub success: bool,
}

impl fmt::Display for ArbitrageExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,
            "ARB {} | buy={} sell={} | expected={:.4}% actual=${:.4} | latency={}ms | {}",
            self.buy_order.symbol, self.buy_order.exchange, self.sell_order.exchange,
            self.signal_profit_pct, self.actual_profit_usd, self.latency_ms,
            if self.success { "✅ SUCCESS" } else { "❌ FAILED" }
        )
    }
}

// ===== Executor =====

pub struct ExecutionConfig {
    pub dry_run: bool,
    pub min_score: u8,
    pub max_trade_usd: f64,
    pub skip_stale: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self { dry_run: true, min_score: 6, max_trade_usd: 500.0, skip_stale: true }
    }
}

pub struct ArbitrageExecutor {
    config: ExecutionConfig,
    coinbase: CoinbaseExecutor,
    bitget: BitgetExecutor,
    binance: BinanceExecutor,
    bybit: BybitExecutor,
    pub inventory: InventoryManager,
    pub liquidity_recovery: LiquidityRecovery,
    pub total_executions: u64,
    pub total_estimated_usd: f64,
    pub total_actual_usd: f64,
    pub trade_log: Vec<TradeResult>,
    /// Cooldown: (buy_exchange, sell_exchange, symbol) → last_exec_ms
    last_exec: std::collections::HashMap<(String, String, String), u64>,
    /// Stuck positions: (exchange, symbol) → (quantity, stuck_since_ms, retry_count)
    stuck_positions: std::collections::HashMap<(String, String), (f64, u64, u32)>,
    /// Shared DB handle для записи liquidity_sales
    db: Option<Arc<Database>>,
}

impl ArbitrageExecutor {
    pub fn new(
        config: ExecutionConfig,
        db: Option<Arc<Database>>,
        coinbase_key: String, coinbase_secret: String,
        bitget_key: String, bitget_secret: String, bitget_pass: String,
        binance_key: String, binance_secret: String,
        bybit_key: String, bybit_secret: String,
    ) -> Self {
        let dry = config.dry_run;
        Self {
            coinbase: CoinbaseExecutor::new(coinbase_key, coinbase_secret, dry),
            bitget:   BitgetExecutor::new(bitget_key, bitget_secret, bitget_pass, dry),
            binance:  BinanceExecutor::new(binance_key, binance_secret, dry),
            bybit:    BybitExecutor::new(bybit_key, bybit_secret, dry),
            inventory: InventoryManager::new(),
            liquidity_recovery: LiquidityRecovery::new(),
            config,
            total_executions: 0,
            total_estimated_usd: 0.0,
            total_actual_usd: 0.0,
            trade_log: Vec::new(),
            last_exec: std::collections::HashMap::new(),
            stuck_positions: std::collections::HashMap::new(),
            db,
        }
    }

    /// Загрузить реальные балансы со всех бирж (крипта + стейблкоины)
    pub async fn sync_balances(&mut self) {
        info!("📦 [INV] syncing balances from exchanges...");

        let coinbase_balances = self.coinbase.get_all_balances().await;
        for (asset, amount) in &coinbase_balances {
            self.inventory.set_balance("coinbase", asset, *amount);
        }

        let binance_balances = self.binance.get_all_balances().await;
        for (asset, amount) in &binance_balances {
            self.inventory.set_balance("binance", asset, *amount);
        }

        let bitget_balances = self.bitget.get_all_balances().await;
        for (asset, amount) in &bitget_balances {
            self.inventory.set_balance("bitget", asset, *amount);
        }

        let bybit_balances = self.bybit.get_all_balances().await;
        for (asset, amount) in &bybit_balances {
            self.inventory.set_balance("bybit", asset, *amount);
        }

        self.inventory.mark_synced();
        info!("📦 [INV] balances synced:");
        self.inventory.log_all();
        self.check_rebalance();
    }

    /// Проверяем дисбаланс между биржами и логируем предупреждения
    pub fn check_rebalance(&self) {
        let exchanges = ["coinbase", "binance", "bitget", "bybit"];
        let symbols = crate::config::SYMBOLS; // ["ETH", "SOL", "XRP"]

        // Примерные цены для оценки крипто-балансов в USD
        let approx_price = |sym: &str| -> f64 {
            match sym { "BTC" => 70000.0, "ETH" => 1800.0, "SOL" => 140.0, "XRP" => 2.0, "BNB" => 600.0, _ => 1.0 }
        };

        // Считаем полный баланс каждой биржи (стейбл + крипто)
        let mut totals: Vec<(&str, f64, f64)> = vec![]; // (exchange, stable, total)
        for exch in &exchanges {
            let stable = self.inventory.get_stablecoin_balance(exch);
            let mut crypto_usd = 0.0;
            for sym in symbols {
                crypto_usd += self.inventory.get_crypto_balance(exch, sym) * approx_price(sym);
            }
            let total = stable + crypto_usd;
            totals.push((exch, stable, total));
        }

        let grand_total: f64 = totals.iter().map(|(_, _, t)| t).sum();
        if grand_total < 10.0 { return; }

        let target = grand_total / exchanges.len() as f64;

        let max_dev = totals.iter()
            .map(|(_, _, t)| (t - target).abs() / target * 100.0)
            .fold(0.0_f64, f64::max);

        let min_ex = totals.iter().min_by(|a, b| a.2.partial_cmp(&b.2).unwrap()).unwrap();
        let max_ex = totals.iter().max_by(|a, b| a.2.partial_cmp(&b.2).unwrap()).unwrap();

        if max_dev > 60.0 {
            error!(
                "🔴 [REBALANCE] CRITICAL imbalance! {} ${:.2} vs {} ${:.2} (target ${:.2}). Manual rebalance needed!",
                min_ex.0, min_ex.2, max_ex.0, max_ex.2, target
            );
        } else if max_dev > 40.0 {
            warn!(
                "🟡 [REBALANCE] significant imbalance: {} ${:.2} vs {} ${:.2} (target ${:.2}). Consider rebalancing.",
                min_ex.0, min_ex.2, max_ex.0, max_ex.2, target
            );
        } else {
            info!(
                "🟢 [REBALANCE] balanced: {} ${:.2} | {} ${:.2} | {} ${:.2} (target ${:.2}, dev {:.0}%)",
                totals[0].0, totals[0].2,
                totals[1].0, totals[1].2,
                totals[2].0, totals[2].2,
                target, max_dev
            );
        }

        // Предупреждение если стейблкоинов мало (не может купить)
        for (exch, stable, total) in &totals {
            if *stable < 5.0 && *total > 20.0 {
                warn!(
                    "🟡 [REBALANCE] {} has only ${:.2} stablecoin (total ${:.2}) — can't BUY on this exchange",
                    exch, stable, total
                );
            }
        }
    }

    /// Inventory optimizer: выравнивает портфель на каждой бирже
    /// Цель: ~50% стейблкоинов + ~16.7% каждая из 3 монет (ETH, SOL, XRP)
    pub async fn inventory_optimize(&mut self) {
        if self.config.dry_run {
            info!("[INV-OPT] skipping in dry_run mode");
            return;
        }

        let target_assets = crate::config::SYMBOLS; // ["ETH", "SOL", "XRP"]
        let exchanges = ["coinbase", "binance", "bitget", "bybit"];

        let approx_price = |sym: &str| -> f64 {
            match sym { "BTC" => 70000.0, "ETH" => 2100.0, "SOL" => 140.0, "XRP" => 1.35, "BNB" => 600.0, _ => 1.0 }
        };

        for exchange in &exchanges {
            let stable_bal = self.inventory.get_stablecoin_balance(exchange);

            // Считаем текущую стоимость крипты на бирже
            let mut crypto_values: Vec<(&str, f64)> = vec![];
            let mut total_crypto_usd = 0.0;
            for asset in target_assets {
                let qty = self.inventory.get_crypto_balance(exchange, asset);
                let value_usd = qty * approx_price(asset);
                crypto_values.push((asset, value_usd));
                total_crypto_usd += value_usd;
            }

            let total_usd = stable_bal + total_crypto_usd;
            if total_usd < 30.0 {
                warn!("[INV-OPT] {} — total ${:.2} too low, skipping", exchange, total_usd);
                continue;
            }

            // Целевое распределение: 50% стейбл, 50% крипто (поровну)
            let target_crypto_each = total_usd * 0.5 / target_assets.len() as f64;

            // Считаем сколько нужно докупить каждого актива
            let mut buys: Vec<(&str, f64)> = vec![];
            for (asset, current_usd) in &crypto_values {
                let deficit = target_crypto_each - current_usd;
                if deficit > 10.0 { // минимум $10 — Bybit отклоняет ордера меньше ~$5-10
                    buys.push((asset, deficit));
                }
            }

            if buys.is_empty() {
                info!(
                    "[INV-OPT] {} — balanced ✅ (stable ${:.2} | {} ${:.2} | {} ${:.2} | {} ${:.2})",
                    exchange, stable_bal,
                    crypto_values[0].0, crypto_values[0].1,
                    crypto_values[1].0, crypto_values[1].1,
                    crypto_values[2].0, crypto_values[2].1,
                );
                continue;
            }

            // Проверяем что хватит стейблкоинов
            let total_buy: f64 = buys.iter().map(|(_, v)| v).sum();
            let available = stable_bal * 0.95; // оставляем 5% запас
            let scale = if total_buy > available { available / total_buy } else { 1.0 };

            info!(
                "[INV-OPT] {} — rebalancing (total ${:.2}, target/asset ${:.2}, stable ${:.2})",
                exchange, total_usd, target_crypto_each, stable_bal
            );

            for (asset, deficit) in &buys {
                let buy_usd = deficit * scale;
                if buy_usd < 10.0 { continue; }

                info!(
                    "[INV-OPT] buying ~${:.2} of {} on {} (current ${:.2}, target ${:.2})",
                    buy_usd, asset, exchange,
                    crypto_values.iter().find(|(a, _)| a == asset).map(|(_, v)| *v).unwrap_or(0.0),
                    target_crypto_each,
                );

                let result = match *exchange {
                    "coinbase" => {
                        let quantity = self.estimate_quantity(asset, buy_usd);
                        if quantity <= 0.0 { continue; }
                        let mut buy_order = Order::market_buy(exchange, asset, quantity);
                        buy_order.price = buy_usd / quantity;
                        self.execute_on(exchange, &mut buy_order).await
                    }
                    "binance" => {
                        self.binance.buy_by_quote(asset, buy_usd).await
                    }
                    "bitget" => {
                        self.bitget.buy_by_quote(asset, buy_usd).await
                    }
                    "bybit" => {
                        self.bybit.buy_by_quote(asset, buy_usd).await
                    }
                    _ => continue,
                };

                match result {
                    Ok(_) => info!("[INV-OPT] ✅ bought {} on {}", asset, exchange),
                    Err(e) => warn!("[INV-OPT] ❌ failed to buy {} on {}: {}", asset, exchange, e),
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }

        // Синхронизируем после всех покупок
        self.sync_balances().await;
    }

    /// Примерный расчёт количества монет для покупки на сумму USD
    fn estimate_quantity(&self, symbol: &str, usd_amount: f64) -> f64 {
        // Примерные цены — будут скорректированы market order'ом
        let approx_price = match symbol {
            "BTC" => 70000.0,
            "ETH" => 1800.0,
            "SOL" => 140.0,
            "XRP" => 2.0,
            "BNB" => 600.0,
            _ => return 0.0,
        };

        let raw_qty = usd_amount / approx_price;

        // Округляем как в try_execute
        match symbol {
            "BTC" => (raw_qty * 100000.0).floor() / 100000.0,
            "ETH" => (raw_qty * 10000.0).floor() / 10000.0,
            "SOL" => (raw_qty * 1000.0).floor() / 1000.0,
            "XRP" => (raw_qty * 10.0).floor() / 10.0,
            "BNB" => (raw_qty * 1000.0).floor() / 1000.0,
            _     => (raw_qty * 100.0).floor() / 100.0,
        }
    }

    /// Исполнение ордера на любой бирже
    async fn execute_on(&self, exchange: &str, order: &mut Order) -> Result<()> {
        match exchange {
            "coinbase" => self.coinbase.execute(order).await,
            "binance"  => self.binance.execute(order).await,
            "bitget"   => self.bitget.execute(order).await,
            "bybit"    => self.bybit.execute(order).await,
            other      => Err(anyhow::anyhow!("unknown exchange: {}", other)),
        }
    }

    pub async fn try_execute(&mut self, opp: &ArbitrageOpportunity) -> Option<ArbitrageExecution> {
        // Фильтры
        if opp.signal_score < self.config.min_score {
            return None; // тихий skip — слишком частый
        }
        if self.config.skip_stale && opp.is_stale {
            return None;
        }

        // Только наши биржи
        let our = ["coinbase", "binance", "bitget", "bybit"];
        if !our.contains(&opp.buy_exchange.as_str()) || !our.contains(&opp.sell_exchange.as_str()) {
            return None;
        }

        // ===== COOLDOWN 30 сек =====
        let cooldown_key = (opp.buy_exchange.clone(), opp.sell_exchange.clone(), opp.symbol.clone());
        let now = now_ms();
        if let Some(&last) = self.last_exec.get(&cooldown_key) {
            if now - last < 30_000 {
                return None; // тихий skip — cooldown
            }
        }

        // Рассчитываем размер сделки
        let mut trade_usd = opp.max_size_usd.min(self.config.max_trade_usd);

        // Адаптируем размер под доступные балансы
        let stablecoin_bal = self.inventory.get_stablecoin_balance(&opp.buy_exchange);
        if stablecoin_bal < trade_usd {
            trade_usd = (stablecoin_bal * 0.95).max(0.0);
        }
        let crypto_bal = self.inventory.get_crypto_balance(&opp.sell_exchange, &opp.symbol);
        let crypto_val_usd = crypto_bal * opp.sell_price;
        if crypto_val_usd < trade_usd {
            trade_usd = (crypto_val_usd * 0.95).max(0.0);
        }

        if trade_usd < 5.0 {
            tracing::info!(
                "[exec] skip: {} {}→{} trade=${:.2} too small (stable=${:.2} crypto={:.4})",
                opp.symbol, opp.buy_exchange, opp.sell_exchange,
                trade_usd, stablecoin_bal, crypto_bal,
            );
            return None;
        }

        // Округляем quantity
        let raw_qty = trade_usd / opp.buy_price;
        let quantity = match opp.symbol.as_str() {
            "BTC" => (raw_qty * 100000.0).floor() / 100000.0,
            "ETH" => (raw_qty * 10000.0).floor()  / 10000.0,
            "SOL" => (raw_qty * 1000.0).floor()   / 1000.0,
            "XRP" => (raw_qty * 10.0).floor()     / 10.0,
            "BNB" => (raw_qty * 1000.0).floor()   / 1000.0,
            _     => (raw_qty * 100.0).floor()    / 100.0,
        };
        if quantity <= 0.0 { return None; }

        let estimated_profit_usd = trade_usd * opp.estimated_profit_pct / 100.0;

        // Фильтр: минимальная прибыль $0.005 (0.01% от $50)
        if estimated_profit_usd < 0.005 {
            tracing::debug!(
                "[exec] skip: {} {}→{} est_profit=${:.4} < $0.005 min",
                opp.symbol, opp.buy_exchange, opp.sell_exchange, estimated_profit_usd
            );
            return None;
        }

        info!(
            "⚡ [EXEC] {} {}→{} profit={:.4}% est=${:.4} qty={:.4} trade=${:.2} {}",
            opp.symbol, opp.buy_exchange, opp.sell_exchange,
            opp.estimated_profit_pct, estimated_profit_usd, quantity, trade_usd,
            if self.config.dry_run { "[DRY RUN]" } else { "[LIVE]" }
        );

        // Ставим cooldown СРАЗУ — до исполнения, чтобы следующие тики не пролезли
        self.last_exec.insert(cooldown_key.clone(), now);

        let signal_ts = opp.timestamp_ms;
        let mut buy_order  = Order::market_buy(&opp.buy_exchange, &opp.symbol, quantity);
        buy_order.price    = opp.buy_price;
        let mut sell_order = Order::market_sell(&opp.sell_exchange, &opp.symbol, quantity);
        sell_order.price   = opp.sell_price;

        // ===== PRE-FLIGHT: проверяем реальный баланс перед BUY =====
        if !self.config.dry_run && opp.buy_exchange == "coinbase" {
            let required_usdc = opp.buy_price * quantity * 1.02;
            let real_balance = self.coinbase.get_balance_usd().await;
            if real_balance < required_usdc {
                warn!(
                    "[EXEC] pre-flight SKIP: coinbase balance {:.2} < required {:.2} for {} qty={:.4}",
                    real_balance, required_usdc, opp.symbol, quantity
                );
                self.inventory.set_balance("coinbase", "USDC", real_balance);
                return None;
            }
        }

        // ===== SEQUENTIAL: сначала BUY, потом SELL =====
        let buy_result = self.execute_on(&opp.buy_exchange, &mut buy_order).await;

        if let Err(e) = &buy_result {
            error!("[EXEC] buy failed on {}: {e}", opp.buy_exchange);
            warn!("[EXEC] ❌ buy failed → NOT sending sell. No damage.");
            if !self.config.dry_run {
                self.sync_balances().await;
            }
            return Some(ArbitrageExecution {
                buy_order, sell_order,
                signal_profit_pct: opp.estimated_profit_pct,
                actual_profit_usd: 0.0,
                latency_ms: now_ms() - signal_ts,
                success: false,
            });
        }

        info!("[EXEC] ✅ buy OK on {} → sending sell on {}", opp.buy_exchange, opp.sell_exchange);
        let sell_result = self.execute_on(&opp.sell_exchange, &mut sell_order).await;

        let latency_ms = now_ms() - signal_ts;
        let success = sell_result.is_ok();

        let actual_profit_usd = if success {
            let buy_cost  = buy_order.quantity * opp.buy_price * (1.0 + 0.0005);
            let sell_recv = sell_order.quantity * opp.sell_price * (1.0 - 0.0010);

            self.inventory.record_execution(
                &opp.buy_exchange, &opp.sell_exchange, &opp.symbol,
                buy_cost, sell_recv, quantity,
            );

            let buy_fee = match opp.buy_exchange.as_str() {
                "coinbase" => 0.0005,
                _ => 0.0010,
            };
            let sell_fee = match opp.sell_exchange.as_str() {
                "coinbase" => 0.0005,
                _ => 0.0010,
            };
            self.liquidity_recovery.record_buy(
                &opp.buy_exchange, &opp.symbol,
                quantity, opp.buy_price,
                buy_fee, sell_fee,
            );

            sell_recv - buy_cost
        } else {
            if let Err(e) = &sell_result {
                error!("[EXEC] sell failed on {}: {e}", opp.sell_exchange);
            }
            let buy_cost = buy_order.quantity * opp.buy_price * (1.0 + 0.0005);
            self.inventory.record_partial_buy(
                &opp.buy_exchange, &opp.symbol, buy_cost, quantity,
            );
            warn!("[EXEC] ⚠️ BUY ok, SELL fail! {} +{:.4} {} stuck on {}",
                opp.symbol, quantity, opp.symbol, opp.buy_exchange);
            let stuck_key = (opp.buy_exchange.clone(), opp.symbol.clone());
            let entry = self.stuck_positions.entry(stuck_key).or_insert((0.0, now_ms(), 0));
            entry.0 += quantity;
            warn!("[STUCK] tracked: {} {:.4} {} (total stuck: {:.4})",
                opp.buy_exchange, quantity, opp.symbol, entry.0);
            0.0
        };

        let slippage_pct = if success && estimated_profit_usd > 0.0 {
            (estimated_profit_usd - actual_profit_usd) / estimated_profit_usd * 100.0
        } else { 0.0 };

        if success {
            self.total_executions += 1;
            self.total_estimated_usd += estimated_profit_usd;
            self.total_actual_usd    += actual_profit_usd;

            info!(
                "✅ [EXEC] done | est=${:.4} actual=${:.4} slippage={:.1}% latency={}ms total=${:.4}",
                estimated_profit_usd, actual_profit_usd, slippage_pct, latency_ms, self.total_actual_usd
            );

            self.trade_log.push(TradeResult {
                timestamp_ms: opp.timestamp_ms,
                buy_exchange: opp.buy_exchange.clone(),
                sell_exchange: opp.sell_exchange.clone(),
                symbol: opp.symbol.clone(),
                estimated_profit_pct: opp.estimated_profit_pct,
                actual_profit_usd,
                slippage_pct,
                latency_ms,
                success,
            });
        } else {
            warn!("[EXEC] execution failed latency={}ms", latency_ms);
        }

        // Синхронизация балансов после сделки
        if !self.config.dry_run {
            self.sync_balances().await;
        }

        Some(ArbitrageExecution {
            buy_order, sell_order,
            signal_profit_pct: opp.estimated_profit_pct,
            actual_profit_usd,
            latency_ms,
            success,
        })
    }

    /// Попытаться продать застрявшую крипту (вызывается периодически)
    pub async fn try_exit_stuck(&mut self) {
        if self.config.dry_run { return; }

        let now = now_ms();
        let mut to_remove = vec![];

        let candidates: Vec<_> = self.stuck_positions.iter()
            .filter(|(_, (qty, since, retries))| {
                *qty > 0.0 && now - *since > 30_000 && *retries < 5
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for ((exchange, symbol), (qty, _since, retries)) in candidates {
            info!("[STUCK-EXIT] attempting market sell: {} {:.4} {} (retry #{})",
                exchange, qty, symbol, retries + 1);

            let mut sell_order = Order::market_sell(&exchange, &symbol, qty);
            sell_order.price = 0.0;

            let result = self.execute_on(&exchange, &mut sell_order).await;

            if let Some(entry) = self.stuck_positions.get_mut(&(exchange.clone(), symbol.clone())) {
                match result {
                    Ok(_) => {
                        info!("[STUCK-EXIT] ✅ sold {:.4} {} on {} — position cleared",
                            qty, symbol, exchange);

                        let stable_asset = crate::execution::inventory::InventoryManager::stablecoin_for_pub(&exchange);
                        let crypto_bal = self.inventory.get_crypto_balance(&exchange, &symbol);
                        self.inventory.set_balance(&exchange, &symbol, (crypto_bal - qty).max(0.0));
                        let _ = stable_asset;

                        // Записываем STUCK_EXIT в БД
                        // buy_price неизвестна (нет лота для stuck) — пишем 0.0
                        // P&L тоже 0.0, sync_balances покажет реальный результат
                        if let Some(db) = &self.db {
                            if let Err(e) = db.insert_liquidity_sale(
                                now_ms(), "STUCK_EXIT",
                                &exchange, &symbol, qty,
                                0.0, 0.0,   // buy/sell price неизвестны без лота
                                0.0, 0.0,
                                0.0, 0.0,
                                "SUCCESS",
                            ) {
                                error!("[STUCK-EXIT] db write error: {}", e);
                            } else {
                                info!("[STUCK-EXIT] 📊 logged to DB: {} {:.4} {} (P&L unknown — no lot)",
                                    exchange, qty, symbol);
                            }
                        }

                        // Очищаем лот в LiquidityRecovery — иначе он будет пытаться
                        // продать ту же монету снова бесконечно
                        self.liquidity_recovery.clear_lot(&exchange, &symbol);

                        to_remove.push((exchange.clone(), symbol.clone()));
                    }
                    Err(e) => {
                        warn!("[STUCK-EXIT] ❌ sell failed: {} — will retry", e);

                        // Логируем неудачную попытку в БД
                        if let Some(db) = &self.db {
                            let _ = db.insert_liquidity_sale(
                                now_ms(), "STUCK_EXIT",
                                &exchange, &symbol, qty,
                                0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                                "FAILED",
                            );
                        }

                        entry.2 += 1;
                        entry.1 = now;
                    }
                }
            }
        }

        for key in to_remove {
            self.stuck_positions.remove(&key);
        }

        if !self.stuck_positions.is_empty() {
            self.sync_balances().await;
        }

        for ((exchange, symbol), (qty, _since, retries)) in &self.stuck_positions {
            if *qty > 0.0 {
                warn!("[STUCK] remaining: {} {:.4} {} (retries: {}/5)", exchange, qty, symbol, retries);
            }
        }
    }

    /// Проверяем и выполняем возврат ликвидности (вызывается каждую минуту)
    pub async fn try_liquidity_recovery(
        &mut self,
        current_prices: &std::collections::HashMap<(String, String), f64>,
    ) {
        if self.config.dry_run || !self.liquidity_recovery.has_lots() {
            return;
        }

        let approx_price = |sym: &str| -> f64 {
            match sym {
                "ETH" => 1800.0, "SOL" => 140.0, "XRP" => 2.0, _ => 1.0
            }
        };
        let symbols = crate::config::SYMBOLS;

        let mut exchange_totals: std::collections::HashMap<String, (f64, f64)> =
            std::collections::HashMap::new();

        for exch in &["coinbase", "binance", "bitget", "bybit"] {
            let stable = self.inventory.get_stablecoin_balance(exch);
            let mut crypto_usd = 0.0;
            for sym in symbols {
                let price = current_prices
                    .get(&(exch.to_string(), sym.to_string()))
                    .copied()
                    .unwrap_or_else(|| approx_price(sym));
                crypto_usd += self.inventory.get_crypto_balance(exch, sym) * price;
            }
            let total = stable + crypto_usd;
            exchange_totals.insert(exch.to_string(), (stable, total));

            if total > 5.0 {
                let ratio = stable / total * 100.0;
                info!("💧 [LIQ] {} stable={:.0}% (${:.2} / ${:.2})", exch, ratio, stable, total);
            }
        }

        let exits = self.liquidity_recovery.check_exits(
            &self.inventory, // <-- ДОБАВИЛИ ВОТ ЭТУ СТРОКУ
            &exchange_totals,
            current_prices,
            0.30,
            300_000,
        );

        for (exchange, asset, qty) in exits {
            info!("💧 [LIQ-EXIT] selling {:.4} {} on {} to recover stablecoins", qty, asset, exchange);

            // Берём лот ДО продажи — нужен для расчёта P&L
            let lot = self.liquidity_recovery.get_lot(&exchange, &asset);

            let mut sell_order = Order::market_sell(&exchange, &asset, qty);
            sell_order.price = 0.0;

            let result = self.execute_on(&exchange, &mut sell_order).await;

            match result {
                Ok(_) => {
                    info!("💧 [LIQ-EXIT] ✅ sold {:.4} {} on {}", qty, asset, exchange);

                    // Обновляем inventory и очищаем лот
                    let crypto_bal = self.inventory.get_crypto_balance(&exchange, &asset);
                    self.inventory.set_balance(&exchange, &asset, (crypto_bal - qty).max(0.0));
                    self.liquidity_recovery.clear_lot(&exchange, &asset);

                    // Записываем в БД с реальным P&L
                    if let Some(db) = &self.db {
                        if let Some(lot) = lot {
                            // Берём live цену продажи из current_prices
                            let sell_price = current_prices
                                .get(&(exchange.clone(), asset.clone()))
                                .copied()
                                .unwrap_or(0.0);

                            let sell_fee = if exchange == "coinbase" { 0.0005 } else { 0.001 };
                            let buy_cost_usd = qty * lot.entry_price * (1.0 + lot.buy_fee_pct);
                            let sell_recv_usd = qty * sell_price * (1.0 - sell_fee);
                            let pnl_usd = sell_recv_usd - buy_cost_usd;
                            let pnl_pct = if buy_cost_usd > 0.0 {
                                pnl_usd / buy_cost_usd * 100.0
                            } else { 0.0 };

                            info!(
                                "💧 [LIQ-EXIT] 📊 P&L: buy@{:.4} sell@{:.4} cost=${:.4} recv=${:.4} pnl=${:.4} ({:.3}%)",
                                lot.entry_price, sell_price, buy_cost_usd, sell_recv_usd, pnl_usd, pnl_pct
                            );

                            if let Err(e) = db.insert_liquidity_sale(
                                now_ms(), "LIQUIDITY_RECOVERY",
                                &exchange, &asset, qty,
                                lot.entry_price, sell_price,
                                buy_cost_usd, sell_recv_usd,
                                pnl_usd, pnl_pct,
                                "SUCCESS",
                            ) {
                                error!("💧 [LIQ-EXIT] db write error: {}", e);
                            }
                        } else {
                            // Лот не найден (не должно случаться, но страхуемся)
                            warn!("💧 [LIQ-EXIT] lot not found for {}/{} — logging without P&L", exchange, asset);
                            let _ = db.insert_liquidity_sale(
                                now_ms(), "LIQUIDITY_RECOVERY",
                                &exchange, &asset, qty,
                                0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                                "SUCCESS",
                            );
                        }
                    }

                    self.sync_balances().await;
                }
                Err(e) => {
                    warn!("💧 [LIQ-EXIT] ❌ sell failed: {} — will retry next cycle", e);

                    // Логируем неудачу в БД
                    if let Some(db) = &self.db {
                        let buy_price = lot.as_ref().map(|l| l.entry_price).unwrap_or(0.0);
                        let sell_price = current_prices
                            .get(&(exchange.clone(), asset.clone()))
                            .copied()
                            .unwrap_or(0.0);
                        let _ = db.insert_liquidity_sale(
                            now_ms(), "LIQUIDITY_RECOVERY",
                            &exchange, &asset, qty,
                            buy_price, sell_price,
                            0.0, 0.0, 0.0, 0.0,
                            "FAILED",
                        );
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        }
    }

    pub fn report(&self) {
        let accuracy = if self.total_estimated_usd > 0.0 {
            self.total_actual_usd / self.total_estimated_usd * 100.0
        } else { 0.0 };
        info!(
            "[exec] trades={} | estimated=${:.4} actual=${:.4} | accuracy={:.1}%",
            self.total_executions, self.total_estimated_usd, self.total_actual_usd, accuracy,
        );
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
