#[allow(dead_code)]
pub struct ExchangeConfig {
    pub name: &'static str,
    pub taker_fee_pct: f64,
}

pub const BINANCE:  ExchangeConfig = ExchangeConfig { name: "binance",  taker_fee_pct: 0.10 };
pub const KRAKEN:   ExchangeConfig = ExchangeConfig { name: "kraken",   taker_fee_pct: 0.26 };
pub const OKX:      ExchangeConfig = ExchangeConfig { name: "okx",      taker_fee_pct: 0.10 };
pub const BYBIT:    ExchangeConfig = ExchangeConfig { name: "bybit",    taker_fee_pct: 0.10 };
pub const GATEIO:   ExchangeConfig = ExchangeConfig { name: "gateio",   taker_fee_pct: 0.20 };
pub const BITGET:   ExchangeConfig = ExchangeConfig { name: "bitget",   taker_fee_pct: 0.10 };
pub const COINBASE: ExchangeConfig = ExchangeConfig { name: "coinbase", taker_fee_pct: 0.05 };

pub fn fee_for(exchange: &str) -> f64 {
    match exchange {
        "binance"  => BINANCE.taker_fee_pct,
        "kraken"   => KRAKEN.taker_fee_pct,
        "okx"      => OKX.taker_fee_pct,
        "bybit"    => BYBIT.taker_fee_pct,
        "gateio"   => GATEIO.taker_fee_pct,
        "bitget"   => BITGET.taker_fee_pct,
        "coinbase" => COINBASE.taker_fee_pct,
        _          => 0.20,
    }
}

pub const SYMBOLS: &[&str] = &["ETH", "SOL", "XRP"];

pub const MIN_PROFIT_PCT_CSV: f64 = 0.01;
pub const MAX_TRADE_USD: f64 = 500.0; // переопределяется через .env в main.rs
pub const MIN_PROFIT_PCT: f64 = -0.30;
pub const STALE_THRESHOLD_MS: u64 = 120000;
