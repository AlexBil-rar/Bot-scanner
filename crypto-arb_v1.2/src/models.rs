#[derive(Debug, Clone)]
pub struct MarketPrice {
    pub exchange: String,
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    pub timestamp_ms: u64,
    /// true = цена в USDT (Binance, Bybit, OKX, Gate, Bitget)
    /// false = цена в USD (Coinbase, Kraken)
    pub is_usdt: bool,
}

#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub buy_exchange: String,
    pub sell_exchange: String,
    pub symbol: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub spread_pct: f64,
    pub estimated_profit_pct: f64,
    pub timestamp_ms: u64,
    pub is_stale: bool,
    pub max_size_usd: f64,
    pub expected_profit_usd: f64,
    pub signal_duration_ms: u64,
    pub tick_count: u32,
    pub signal_score: u8,
    pub signal_status: &'static str,
    pub usdt_rate: f64,         // курс USDT/USD в момент сигнала
    pub raw_spread_pct: f64,    // спред БЕЗ нормализации (для сравнения)
}

/// Результат реальной сделки — сравниваем с приблизительной
#[derive(Debug, Clone)]
pub struct TradeResult {
    pub timestamp_ms: u64,
    pub buy_exchange: String,
    pub sell_exchange: String,
    pub symbol: String,
    pub estimated_profit_pct: f64,  // что мы ожидали
    pub actual_profit_usd: f64,     // что реально получили
    pub slippage_pct: f64,          // проскальзывание
    pub latency_ms: u64,
    pub success: bool,
}
