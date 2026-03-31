use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use crate::config::MIN_PROFIT_PCT_CSV;
use crate::models::ArbitrageOpportunity;

pub struct OpportunityWriter {
    writer: BufWriter<File>,
    path: String,
    count: u64,
    last_written: HashMap<(String, String, String), u64>,
    dedup_window_ms: u64,
}

impl OpportunityWriter {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let is_new = !Path::new(path).exists();
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut writer = BufWriter::new(file);
        if is_new {
            writeln!(writer, "timestamp_ms,buy_exchange,sell_exchange,symbol,buy_price,sell_price,raw_spread_pct,normalized_spread_pct,profit_pct,max_size_usd,expected_profit_usd,signal_duration_ms,tick_count,signal_score,signal_status,usdt_rate,is_stale")?;
            writer.flush()?;
        }
        Ok(Self { writer, path: path.to_string(), count: 0, last_written: HashMap::new(), dedup_window_ms: 1000 })
    }

    pub fn write_if_profitable(&mut self, o: &ArbitrageOpportunity) -> anyhow::Result<bool> {
        if o.estimated_profit_pct < MIN_PROFIT_PCT_CSV { return Ok(false); }
        let key = (o.buy_exchange.clone(), o.sell_exchange.clone(), o.symbol.clone());
        if let Some(&last_ts) = self.last_written.get(&key) {
            if o.timestamp_ms.saturating_sub(last_ts) < self.dedup_window_ms { return Ok(false); }
        }
        self.last_written.insert(key, o.timestamp_ms);
        writeln!(self.writer, "{},{},{},{},{:.4},{:.4},{:.6},{:.6},{:.6},{:.2},{:.4},{},{},{},{},{:.6},{}",
            o.timestamp_ms, o.buy_exchange, o.sell_exchange, o.symbol,
            o.buy_price, o.sell_price, o.raw_spread_pct, o.spread_pct, o.estimated_profit_pct,
            o.max_size_usd, o.expected_profit_usd, o.signal_duration_ms,
            o.tick_count, o.signal_score, o.signal_status, o.usdt_rate, o.is_stale)?;
        self.writer.flush()?;
        self.count += 1;
        Ok(true)
    }

    pub fn count(&self) -> u64 { self.count }
    pub fn path(&self) -> &str { &self.path }
}

pub struct SpreadHistoryWriter {
    writer: BufWriter<File>,
}

impl SpreadHistoryWriter {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let is_new = !Path::new(path).exists();
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut writer = BufWriter::new(file);
        if is_new {
            writeln!(writer, "timestamp_ms,symbol,best_bid_exchange,best_bid,best_ask_exchange,best_ask,raw_spread_pct")?;
            writer.flush()?;
        }
        Ok(Self { writer })
    }

    pub fn write(&mut self, ts: u64, symbol: &str, bid_ex: &str, bid: f64, ask_ex: &str, ask: f64) -> anyhow::Result<()> {
        let raw_spread = (bid - ask) / ask * 100.0;
        writeln!(self.writer, "{},{},{},{:.4},{},{:.4},{:.6}", ts, symbol, bid_ex, bid, ask_ex, ask, raw_spread)?;
        self.writer.flush()?;
        Ok(())
    }
}
