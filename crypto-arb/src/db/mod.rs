use anyhow::Result;
use rusqlite::{Connection, params};
use tracing::{error, info};
use crate::models::{ArbitrageOpportunity, MarketPrice};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        info!("[db] opened: {}", path);
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS opportunities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ms INTEGER NOT NULL,
                buy_exchange TEXT NOT NULL,
                sell_exchange TEXT NOT NULL,
                symbol TEXT NOT NULL,
                buy_price REAL NOT NULL,
                sell_price REAL NOT NULL,
                raw_spread_pct REAL NOT NULL,
                normalized_spread_pct REAL NOT NULL,
                profit_pct REAL NOT NULL,
                max_size_usd REAL NOT NULL,
                expected_profit_usd REAL NOT NULL,
                signal_duration_ms INTEGER NOT NULL,
                tick_count INTEGER NOT NULL,
                signal_score INTEGER NOT NULL,
                signal_status TEXT NOT NULL,
                usdt_rate REAL NOT NULL,
                is_stale INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS spreads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ms INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                best_bid_exchange TEXT NOT NULL,
                best_bid REAL NOT NULL,
                best_ask_exchange TEXT NOT NULL,
                best_ask REAL NOT NULL,
                raw_spread_pct REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS prices (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ms INTEGER NOT NULL,
                exchange TEXT NOT NULL,
                symbol TEXT NOT NULL,
                bid REAL NOT NULL,
                ask REAL NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_opp_ts ON opportunities(timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_opp_symbol ON opportunities(symbol);
            CREATE INDEX IF NOT EXISTS idx_spread_ts ON spreads(timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_price_ts ON prices(timestamp_ms);
        ")?;
        Ok(())
    }

    pub fn insert_opportunity(&self, o: &ArbitrageOpportunity) -> Result<()> {
        self.conn.execute(
            "INSERT INTO opportunities
             (timestamp_ms, buy_exchange, sell_exchange, symbol,
              buy_price, sell_price, raw_spread_pct, normalized_spread_pct,
              profit_pct, max_size_usd, expected_profit_usd,
              signal_duration_ms, tick_count, signal_score, signal_status,
              usdt_rate, is_stale)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                o.timestamp_ms as i64,
                o.buy_exchange, o.sell_exchange, o.symbol,
                o.buy_price, o.sell_price,
                o.raw_spread_pct, o.spread_pct,
                o.estimated_profit_pct,
                o.max_size_usd, o.expected_profit_usd,
                o.signal_duration_ms as i64,
                o.tick_count as i64,
                o.signal_score as i64,
                o.signal_status,
                o.usdt_rate,
                o.is_stale as i64,
            ],
        )?;
        Ok(())
    }

    pub fn insert_spread(
        &self, ts: u64, symbol: &str,
        bid_ex: &str, bid: f64, ask_ex: &str, ask: f64, spread: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO spreads (timestamp_ms, symbol, best_bid_exchange, best_bid, best_ask_exchange, best_ask, raw_spread_pct)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![ts as i64, symbol, bid_ex, bid, ask_ex, ask, spread],
        )?;
        Ok(())
    }

    pub fn insert_price(&self, p: &MarketPrice) -> Result<()> {
        self.conn.execute(
            "INSERT INTO prices (timestamp_ms, exchange, symbol, bid, ask) VALUES (?1,?2,?3,?4,?5)",
            params![p.timestamp_ms as i64, p.exchange, p.symbol, p.bid, p.ask],
        )?;
        Ok(())
    }
}
