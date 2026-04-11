use std::collections::HashMap;
use tracing::{info, warn};

// ===== Liquidity Recovery =====

/// Лот, записанный после BUY в арбитраже
#[derive(Debug, Clone)]
pub struct InventoryLot {
    pub exchange: String,
    pub asset: String,
    pub qty: f64,
    pub entry_price: f64,
    pub buy_fee_pct: f64,
    pub sell_fee_pct: f64,
    pub timestamp_ms: u64,
}

impl InventoryLot {
    /// Цена безубытка (включает обе комиссии)
    pub fn break_even_price(&self) -> f64 {
        self.entry_price * (1.0 + self.buy_fee_pct + self.sell_fee_pct)
    }
}

/// Менеджер лотов для возврата ликвидности
#[derive(Debug, Default)]
pub struct LiquidityRecovery {
    /// (exchange, asset) → лот
    lots: HashMap<(String, String), InventoryLot>,
}

impl LiquidityRecovery {
    pub fn new() -> Self { Self::default() }

    /// Записать лот после успешного BUY
    pub fn record_buy(
        &mut self,
        exchange: &str,
        asset: &str,
        qty: f64,
        entry_price: f64,
        buy_fee_pct: f64,
        sell_fee_pct: f64,
    ) {
        let key = (exchange.to_string(), asset.to_string());
        let existing = self.lots.get(&key);

        // Weighted average если уже есть лот на этой бирже+монете
        let (new_qty, new_price) = if let Some(prev) = existing {
            let total_qty = prev.qty + qty;
            let avg_price = (prev.qty * prev.entry_price + qty * entry_price) / total_qty;
            (total_qty, avg_price)
        } else {
            (qty, entry_price)
        };

        let lot = InventoryLot {
            exchange: exchange.to_string(),
            asset: asset.to_string(),
            qty: new_qty,
            entry_price: new_price,
            buy_fee_pct,
            sell_fee_pct,
            timestamp_ms: now_ms(),
        };

        info!(
            "💧 [LIQ] recorded lot: {} {:.4} {} @ {:.4} (break_even={:.4})",
            exchange, new_qty, asset, new_price, lot.break_even_price()
        );
        self.lots.insert(key, lot);
    }

    /// Получить копию лота без удаления — нужен для расчёта P&L перед clear_lot()
    pub fn get_lot(&self, exchange: &str, asset: &str) -> Option<InventoryLot> {
        self.lots.get(&(exchange.to_string(), asset.to_string())).cloned()
    }

    /// Удалить лот после продажи
    pub fn clear_lot(&mut self, exchange: &str, asset: &str) {
        self.lots.remove(&(exchange.to_string(), asset.to_string()));
        info!("💧 [LIQ] cleared lot: {} {}", exchange, asset);
    }

    /// Обновить qty лота (например, после частичной продажи)
    /// Если qty <= 0 — лот удаляется
    pub fn update_lot_qty(&mut self, exchange: &str, asset: &str, new_qty: f64) {
        let key = (exchange.to_string(), asset.to_string());
        if new_qty <= 0.0001 {
            self.lots.remove(&key);
            info!("💧 [LIQ] lot removed (qty too small): {} {}", exchange, asset);
        } else if let Some(lot) = self.lots.get_mut(&key) {
            info!(
                "💧 [LIQ] lot qty updated: {} {} {:.4} → {:.4}",
                exchange, asset, lot.qty, new_qty
            );
            lot.qty = new_qty;
        }
    }

    /// Проверить: нужно ли продать лот для возврата ликвидности?
    /// Триггер: stable_ratio < порога (цель 50%, триггер 30%)
    /// Выход: только если цена >= break_even (ноль или плюс)
    /// Возвращает (exchange, asset, qty)
    /// Проверить: нужно ли продать лот для возврата ликвидности?
    pub fn check_exits(
        &self,
        inventory: &InventoryManager, // <-- 1. ДОБАВЛЯЕМ ЭТОТ ПАРАМЕТР
        exchange_totals: &HashMap<String, (f64, f64)>,
        current_prices: &HashMap<(String, String), f64>,
        stable_ratio_threshold: f64,
        min_age_ms: u64,             
    ) -> Vec<(String, String, f64)> {
        let now = now_ms();
        let mut exits = vec![];

        for ((exchange, asset), lot) in &self.lots {
            let (stable, total) = exchange_totals
                .get(exchange)
                .copied()
                .unwrap_or((0.0, 0.0));

            let stable_ratio = if total > 1.0 { stable / total } else { 1.0 };

            if stable_ratio >= stable_ratio_threshold {
                continue;
            }

            let age_ms = now - lot.timestamp_ms;
            if age_ms < min_age_ms {
                let secs_left = (min_age_ms - age_ms) / 1000;
                info!(
                    "💧 [LIQ] {} {} — waiting {}s (stable={:.0}% < {:.0}% threshold)",
                    exchange, asset, secs_left,
                    stable_ratio * 100.0, stable_ratio_threshold * 100.0
                );
                continue;
            }

            let break_even = lot.break_even_price();
            let current_price = current_prices
                .get(&(exchange.clone(), asset.clone()))
                .copied()
                .unwrap_or(0.0);

            if current_price <= 0.0 {
                continue;
            }

            // --- 2. НОВЫЙ БЛОК: ПРОВЕРЯЕМ РЕАЛЬНЫЙ БАЛАНС ---
            let actual_balance = inventory.get_balance(exchange, asset);
            let mut qty_to_sell = lot.qty;

            if actual_balance < qty_to_sell {
                tracing::warn!(
                    "💧 [LIQ] Баланс меньше лота: lot={:.4}, actual={:.4} ({} {}). Корректируем объем.",
                    qty_to_sell, actual_balance, exchange, asset
                );
                qty_to_sell = actual_balance;
            }

            // Если остались сущие копейки (пыль), нет смысла тратить на комсу
            if qty_to_sell <= 0.0001 {
                continue;
            }
            // --------------------------------------------------

            if current_price >= break_even {
                info!(
                    "💧 [LIQ] EXIT signal: {} {:.4} {} price={:.4} >= break_even={:.4} (stable={:.0}%)",
                    exchange, qty_to_sell, asset, current_price, break_even, stable_ratio * 100.0
                );
                // 3. ОТПРАВЛЯЕМ СКОРРЕКТИРОВАННЫЙ ОБЪЕМ (qty_to_sell)
                exits.push((exchange.clone(), asset.clone(), qty_to_sell)); 
            } else {
                let pct_away = (break_even - current_price) / break_even * 100.0;
                info!(
                    "💧 [LIQ] waiting: {} {} price={:.4} need={:.4} ({:.3}% away) stable={:.0}%",
                    exchange, asset, current_price, break_even, pct_away, stable_ratio * 100.0
                );
            }
        }

        exits
    }

    /// Есть ли активные лоты?
    pub fn has_lots(&self) -> bool { !self.lots.is_empty() }

    /// Логировать все лоты
    pub fn log_all(&self) {
        if self.lots.is_empty() {
            info!("💧 [LIQ] no active lots");
            return;
        }
        for ((exchange, asset), lot) in &self.lots {
            let age_min = (now_ms() - lot.timestamp_ms) / 60_000;
            info!(
                "💧 [LIQ] lot: {} {} qty={:.4} @ {:.4} break_even={:.4} age={}min",
                exchange, asset, lot.qty, lot.entry_price, lot.break_even_price(), age_min
            );
        }
    }
}

/// Трекер балансов крипты и стейблкоинов на каждой бирже.
///
/// Ключ: (exchange, asset) → доступный баланс.
/// asset = "XRP", "SOL", "ETH", "BTC", "BNB", "USDT", "USDC", "USD"
///
/// При старте заполняется из реальных API-балансов.
/// После каждой сделки обновляется локально.
/// Периодически синхронизируется с биржами.
#[derive(Debug)]
pub struct InventoryManager {
    balances: HashMap<(String, String), f64>,
    last_sync_ms: u64,
    sync_interval_ms: u64,
}

impl InventoryManager {
    pub fn new() -> Self {
        Self {
            balances: HashMap::new(),
            last_sync_ms: 0,
            sync_interval_ms: 60_000, // синхронизация раз в минуту
        }
    }

    /// Устанавливаем баланс (из API или вручную)
    pub fn set_balance(&mut self, exchange: &str, asset: &str, amount: f64) {
        self.balances.insert((exchange.to_string(), asset.to_string()), amount);
    }

    /// Получаем баланс конкретного ассета на конкретной бирже
    pub fn get_balance(&self, exchange: &str, asset: &str) -> f64 {
        *self.balances.get(&(exchange.to_string(), asset.to_string())).unwrap_or(&0.0)
    }

    /// Стейблкоин-баланс на бирже (USDT для Binance/Bitget, USDC/USD для Coinbase)
    pub fn get_stablecoin_balance(&self, exchange: &str) -> f64 {
        match exchange {
            "coinbase" => {
                self.get_balance(exchange, "USDC") + self.get_balance(exchange, "USD")
            }
            _ => self.get_balance(exchange, "USDT"),
        }
    }

    /// Крипто-баланс на бирже (в монетах)
    pub fn get_crypto_balance(&self, exchange: &str, symbol: &str) -> f64 {
        self.get_balance(exchange, symbol)
    }

    /// Проверяем, можно ли выполнить арбитражную пару:
    /// - buy_exchange: нужны стейблкоины >= trade_usd
    /// - sell_exchange: нужна крипта >= quantity монет
    pub fn can_execute(
        &self,
        buy_exchange: &str,
        sell_exchange: &str,
        symbol: &str,
        trade_usd: f64,
        quantity: f64,
    ) -> CanExecuteResult {
        let stablecoin_bal = self.get_stablecoin_balance(buy_exchange);
        let crypto_bal = self.get_crypto_balance(sell_exchange, symbol);

        let need_stablecoin = trade_usd * 1.01; // +1% запас на проскальзывание
        let need_crypto = quantity;

        if stablecoin_bal < need_stablecoin {
            return CanExecuteResult::InsufficientStablecoin {
                exchange: buy_exchange.to_string(),
                have: stablecoin_bal,
                need: need_stablecoin,
            };
        }

        if crypto_bal < need_crypto {
            return CanExecuteResult::InsufficientCrypto {
                exchange: sell_exchange.to_string(),
                symbol: symbol.to_string(),
                have: crypto_bal,
                need: need_crypto,
            };
        }

        CanExecuteResult::Ok
    }

    /// После успешной сделки обновляем локальные балансы:
    /// buy_exchange:  стейблкоин -= trade_usd,  крипта += quantity
    /// sell_exchange: крипта -= quantity,  стейблкоин += trade_usd
    pub fn record_execution(
        &mut self,
        buy_exchange: &str,
        sell_exchange: &str,
        symbol: &str,
        buy_cost_usd: f64,
        sell_proceeds_usd: f64,
        quantity: f64,
    ) {
        // Buy-сторона: тратим стейблкоины, получаем крипту
        let buy_stable_asset = Self::stablecoin_for(buy_exchange);
        let buy_stable_bal = self.get_balance(buy_exchange, buy_stable_asset);
        self.set_balance(buy_exchange, buy_stable_asset, (buy_stable_bal - buy_cost_usd).max(0.0));

        let buy_crypto_bal = self.get_balance(buy_exchange, symbol);
        self.set_balance(buy_exchange, symbol, buy_crypto_bal + quantity);

        // Sell-сторона: тратим крипту, получаем стейблкоины
        let sell_crypto_bal = self.get_balance(sell_exchange, symbol);
        self.set_balance(sell_exchange, symbol, (sell_crypto_bal - quantity).max(0.0));

        let sell_stable_asset = Self::stablecoin_for(sell_exchange);
        let sell_stable_bal = self.get_balance(sell_exchange, sell_stable_asset);
        self.set_balance(sell_exchange, sell_stable_asset, sell_stable_bal + sell_proceeds_usd);

        info!(
            "📦 [INV] {} buy {:.4} {} (${:.2}) | {} sell {:.4} {} (${:.2})",
            buy_exchange, quantity, symbol, buy_cost_usd,
            sell_exchange, quantity, symbol, sell_proceeds_usd,
        );
        self.log_balances(symbol);
    }

    /// Нужна ли синхронизация с биржами?
    pub fn needs_sync(&self) -> bool {
        let now = now_ms();
        now - self.last_sync_ms > self.sync_interval_ms
    }

    /// Отмечаем, что синхронизация произошла
    pub fn mark_synced(&mut self) {
        self.last_sync_ms = now_ms();
    }

    /// Логируем текущие балансы для символа
    pub fn log_balances(&self, symbol: &str) {
        for exchange in &["coinbase", "binance", "bitget"] {
            let stable = self.get_stablecoin_balance(exchange);
            let crypto = self.get_crypto_balance(exchange, symbol);
            info!(
                "   [INV] {:<9} {} {:.4} | stable ${:.2}",
                exchange, symbol, crypto, stable,
            );
        }
    }

    /// Логируем все балансы
    pub fn log_all(&self) {
        let mut sorted: Vec<_> = self.balances.iter().collect();
        sorted.sort_by_key(|(k, _)| (&k.0, &k.1));
        for ((exch, asset), bal) in sorted {
            if *bal > 0.001 {
                info!("   [INV] {:<9} {:<5} = {:.4}", exch, asset, bal);
            }
        }
    }

    /// Частичное исполнение: только buy прошёл (sell не удался)
    /// buy_exchange: стейблкоин -= cost, крипта += quantity
    pub fn record_partial_buy(
        &mut self,
        buy_exchange: &str,
        symbol: &str,
        buy_cost_usd: f64,
        quantity: f64,
    ) {
        let stable = Self::stablecoin_for(buy_exchange);
        let cur_stable = self.get_balance(buy_exchange, stable);
        self.set_balance(buy_exchange, stable, (cur_stable - buy_cost_usd).max(0.0));

        let cur_crypto = self.get_balance(buy_exchange, symbol);
        self.set_balance(buy_exchange, symbol, cur_crypto + quantity);

        warn!(
            "📦 [INV] partial BUY: {} -{:.2} {} +{:.4} {}",
            buy_exchange, buy_cost_usd, stable, quantity, symbol,
        );
    }

    /// Частичное исполнение: только sell прошёл (buy не удался)
    /// sell_exchange: крипта -= quantity, стейблкоин += proceeds
    pub fn record_partial_sell(
        &mut self,
        sell_exchange: &str,
        symbol: &str,
        sell_proceeds_usd: f64,
        quantity: f64,
    ) {
        let cur_crypto = self.get_balance(sell_exchange, symbol);
        self.set_balance(sell_exchange, symbol, (cur_crypto - quantity).max(0.0));

        let stable = Self::stablecoin_for(sell_exchange);
        let cur_stable = self.get_balance(sell_exchange, stable);
        self.set_balance(sell_exchange, stable, cur_stable + sell_proceeds_usd);

        warn!(
            "📦 [INV] partial SELL: {} -{:.4} {} +{:.2} {}",
            sell_exchange, quantity, symbol, sell_proceeds_usd, stable,
        );
    }

    fn stablecoin_for(exchange: &str) -> &'static str {
        match exchange {
            "coinbase" => "USDC",
            _ => "USDT",
        }
    }

    /// Публичная версия для использования из executor
    pub fn stablecoin_for_pub(exchange: &str) -> &'static str {
        Self::stablecoin_for(exchange)
    }
}

#[derive(Debug)]
pub enum CanExecuteResult {
    Ok,
    InsufficientStablecoin {
        exchange: String,
        have: f64,
        need: f64,
    },
    InsufficientCrypto {
        exchange: String,
        symbol: String,
        have: f64,
        need: f64,
    },
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
