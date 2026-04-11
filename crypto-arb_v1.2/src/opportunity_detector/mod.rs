use std::collections::HashMap;
use tracing::info;

use crate::config::{fee_for, MIN_PROFIT_PCT, MIN_PROFIT_PCT_CSV, STALE_THRESHOLD_MS, SYMBOLS};
use crate::models::{ArbitrageOpportunity, MarketPrice};

/// Минимальный объём сделки в USD — ниже этого окно считается фиктивным
const MIN_SIZE_USD: f64 = 100.0;

/// Отслеживаем активные окна для измерения signal duration
#[derive(Debug, Clone)]
struct ActiveWindow {
    first_seen_ms: u64,
    last_seen_ms: u64,
    max_profit_pct: f64,
    count: u32,
}

pub struct SpreadSnapshot {
    pub symbol: String,
    pub best_bid_exchange: String,
    pub best_bid: f64,
    pub best_ask_exchange: String,
    pub best_ask: f64,
    pub timestamp_ms: u64,
}

pub struct OpportunityDetector {
    prices: HashMap<(String, String), MarketPrice>,
    last_spread_log: u64,
    active_windows: HashMap<(String, String, String), ActiveWindow>,
    /// Текущий курс USDT/USD (атомарный, обновляется из отдельного таска)
    pub usdt_rate: crate::market_data::usdt_usd::UsdtRate,
}

impl OpportunityDetector {
    pub fn new(usdt_rate: crate::market_data::usdt_usd::UsdtRate) -> Self {
        Self {
            prices: HashMap::new(),
            last_spread_log: 0,
            active_windows: HashMap::new(),
            usdt_rate,
        }
    }

    pub fn update(&mut self, price: MarketPrice) -> (Vec<ArbitrageOpportunity>, Option<SpreadSnapshot>) {
        let symbol = price.symbol.clone();
        self.prices.insert((price.exchange.clone(), price.symbol.clone()), price);
        let opps = self.detect_for_symbol(&symbol);
        self.cleanup_closed_windows(&symbol);
        let snapshot = self.maybe_snapshot(&symbol);
        (opps, snapshot)
    }

    fn detect_for_symbol(&mut self, symbol: &str) -> Vec<ArbitrageOpportunity> {
        let prices: Vec<&MarketPrice> = self.prices.values()
            .filter(|p| p.symbol == symbol)
            .collect();

        if prices.len() < 2 { return vec![]; }

        let mut opportunities = vec![];
        let now = now_ms();

        // Текущий курс USDT/USD для нормализации
        let usdt_rate = crate::market_data::usdt_usd::get_rate(&self.usdt_rate);

        for i in 0..prices.len() {
            for j in 0..prices.len() {
                if i == j { continue; }
                let buy = prices[i];
                let sell = prices[j];

                // Нормализуем цены к USD
                // USDT биржа: bid_usd = bid_usdt * usdt_rate
                // USD биржа: bid_usd = bid
                let buy_ask_usd = if buy.is_usdt { buy.ask * usdt_rate } else { buy.ask };
                let sell_bid_usd = if sell.is_usdt { sell.bid * usdt_rate } else { sell.bid };

                if buy_ask_usd >= sell_bid_usd { continue; }

                // Volume filter — только если обе биржи дают размер
                let size_known = buy.ask_size > 0.0 && sell.bid_size > 0.0;
                let max_coins = if size_known {
                    buy.ask_size.min(sell.bid_size)
                } else {
                    // Размер неизвестен (напр. Coinbase) — не фильтруем,
                    // ставим условный размер $1000 для расчёта expected_profit
                    1000.0 / buy.ask
                };
                let max_size_usd = max_coins * buy.ask;

                // Фильтруем только когда размер известен и слишком мал
                if size_known && max_size_usd < MIN_SIZE_USD { continue; }

                // Depth filter: если стакан меньше нашего trade_size — сигнал неисполним
                // Не фильтруем Coinbase (не даёт size)
                let trade_size = crate::config::MAX_TRADE_USD;
                if size_known && max_size_usd < trade_size * 0.5 { continue; }

                let raw_spread_pct = (sell.bid - buy.ask) / buy.ask * 100.0;
                let spread_pct = (sell_bid_usd - buy_ask_usd) / buy_ask_usd * 100.0;
                let buy_fee = fee_for(&buy.exchange);
                let sell_fee = fee_for(&sell.exchange);
                let estimated_profit_pct = spread_pct - buy_fee - sell_fee;

                if estimated_profit_pct < MIN_PROFIT_PCT { continue; }

                let ts_diff = buy.timestamp_ms.abs_diff(sell.timestamp_ms);
                let is_stale = ts_diff > STALE_THRESHOLD_MS;

                let expected_profit_usd = max_size_usd * estimated_profit_pct / 100.0;

                // Signal duration tracking
                let window_key = (buy.exchange.clone(), sell.exchange.clone(), symbol.to_string());
                let window = self.active_windows.entry(window_key.clone()).or_insert(ActiveWindow {
                    first_seen_ms: now, last_seen_ms: now,
                    max_profit_pct: estimated_profit_pct, count: 0,
                });
                window.last_seen_ms = now;
                window.count += 1;
                if estimated_profit_pct > window.max_profit_pct {
                    window.max_profit_pct = estimated_profit_pct;
                }
                let duration_ms = now - window.first_seen_ms;

                let (signal_score, signal_status) = calc_score(
                    estimated_profit_pct, duration_ms, window.count, is_stale
                );

                let opp = ArbitrageOpportunity {
                    buy_exchange: buy.exchange.clone(),
                    sell_exchange: sell.exchange.clone(),
                    symbol: symbol.to_string(),
                    buy_price: buy.ask,
                    sell_price: sell.bid,
                    spread_pct,
                    estimated_profit_pct,
                    timestamp_ms: now,
                    is_stale,
                    max_size_usd,
                    expected_profit_usd,
                    signal_duration_ms: duration_ms,
                    tick_count: window.count,
                    signal_score,
                    signal_status,
                    usdt_rate,
                    raw_spread_pct,
                };

                if estimated_profit_pct >= MIN_PROFIT_PCT_CSV {
                    let stale_mark = if is_stale { "⚠️ STALE" } else { "✅" };
                    info!(
                        "🔥 [OPP] {symbol} {}→{} profit={:.4}% ${:.2} dur={}ms ticks={} score={}/10 [{}] {}",
                        opp.buy_exchange, opp.sell_exchange,
                        opp.estimated_profit_pct, expected_profit_usd,
                        duration_ms, window.count, signal_score, signal_status, stale_mark
                    );
                }
                // [near] окна не логируем — только шум

                opportunities.push(opp);
            }
        }

        opportunities
    }

    /// Закрываем окна которые не обновлялись > 1 сек
    fn cleanup_closed_windows(&mut self, symbol: &str) {
        let now = now_ms();
        let closed: Vec<_> = self.active_windows.iter()
            .filter(|(k, w)| k.2 == symbol && now - w.last_seen_ms > 1000)
            .map(|(k, w)| (k.clone(), w.clone()))
            .collect();

        for (key, w) in closed {
            let duration_ms = w.last_seen_ms - w.first_seen_ms;
            if w.max_profit_pct >= MIN_PROFIT_PCT_CSV {
                info!(
                    "📊 [CLOSED] {}→{} {} | duration={}ms max_profit={:.4}% seen={}x",
                    key.0, key.1, key.2,
                    duration_ms, w.max_profit_pct, w.count
                );
            }
            self.active_windows.remove(&key);
        }
    }

    fn maybe_snapshot(&mut self, symbol: &str) -> Option<SpreadSnapshot> {
        if symbol != SYMBOLS[0] { return None; }
        let now = now_ms();
        if now - self.last_spread_log < 30_000 { return None; }
        self.last_spread_log = now;

        for sym in SYMBOLS {
            let prices: Vec<&MarketPrice> = self.prices.values()
                .filter(|p| p.symbol == *sym)
                .collect();
            if prices.len() < 2 { continue; }

            let best_bid = prices.iter().map(|p| (p.bid, p.exchange.as_str())).fold((0.0_f64, ""), |a, b| if b.0 > a.0 { b } else { a });
            let best_ask = prices.iter().map(|p| (p.ask, p.exchange.as_str())).fold((f64::MAX, ""), |a, b| if b.0 < a.0 { b } else { a });
            let raw_spread = (best_bid.0 - best_ask.0) / best_ask.0 * 100.0;
            let needed = fee_for(best_bid.1) + fee_for(best_ask.1);

            // Топ пары по прибыльности
            let mut pairs: Vec<(f64, String)> = vec![];
            for pb in &prices {
                for ps in &prices {
                    if pb.exchange == ps.exchange { continue; }
                    if pb.ask >= ps.bid { continue; }
                    let s = (ps.bid - pb.ask) / pb.ask * 100.0;
                    let p = s - fee_for(&pb.exchange) - fee_for(&ps.exchange);
                    let max_sz = if pb.ask_size > 0.0 && ps.bid_size > 0.0 {
                        pb.ask_size.min(ps.bid_size) * pb.ask
                    } else { 0.0 };
                    pairs.push((p, format!("{}->{} sz=${:.0}", pb.exchange, ps.exchange, max_sz)));
                }
            }
            pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

            info!("[spread] {sym} raw={:.4}% need={:.2}% gap={:+.4}%", raw_spread, needed, raw_spread - needed);
            for (profit, desc) in pairs.iter().take(3) {
                info!("   {sym} {desc} profit={:+.4}%", profit);
            }
        }

        let btc: Vec<&MarketPrice> = self.prices.values().filter(|p| p.symbol == "BTC").collect();
        if btc.len() < 2 { return None; }
        let best_bid = btc.iter().map(|p| (p.bid, p.exchange.as_str())).fold((0.0_f64, ""), |a, b| if b.0 > a.0 { b } else { a });
        let best_ask = btc.iter().map(|p| (p.ask, p.exchange.as_str())).fold((f64::MAX, ""), |a, b| if b.0 < a.0 { b } else { a });

        Some(SpreadSnapshot {
            symbol: "BTC".to_string(),
            best_bid_exchange: best_bid.1.to_string(), best_bid: best_bid.0,
            best_ask_exchange: best_ask.1.to_string(), best_ask: best_ask.0,
            timestamp_ms: now,
        })
    }
}

fn calc_score(profit_pct: f64, duration_ms: u64, tick_count: u32, is_stale: bool) -> (u8, &'static str) {
    let mut score: u8 = 0;

    // Прибыль
    if profit_pct >= 0.05 { score += 3; }
    else if profit_pct >= 0.03 { score += 2; }
    else if profit_pct >= 0.01 { score += 1; }

    // Длительность
    if duration_ms >= 500 { score += 3; }
    else if duration_ms >= 200 { score += 2; }
    else if duration_ms >= 100 { score += 1; }

    // Повторяемость тиков
    if tick_count >= 10 { score += 2; }
    else if tick_count >= 3  { score += 1; }

    // Штраф за stale
    if is_stale { score = score.saturating_sub(2); }

    let status = if score >= 6 { "strong" }
                 else if score >= 3 { "medium" }
                 else { "weak" };

    (score, status)
}


fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
