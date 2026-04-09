use crate::models::ArbitrageOpportunity;
use tracing::info;

pub struct Stats {
    pub total_opportunities: u64,
    pub stale_opportunities: u64,
    pub max_spread_pct: f64,
    pub max_profit_pct: f64,
    pub binance_updates: u64,
    pub kraken_updates: u64,
    // Signal duration distribution
    durations: Vec<u64>,
    // Profit distribution
    profits: Vec<f64>,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            total_opportunities: 0,
            stale_opportunities: 0,
            max_spread_pct: 0.0,
            max_profit_pct: 0.0,
            binance_updates: 0,
            kraken_updates: 0,
            durations: Vec::new(),
            profits: Vec::new(),
        }
    }

    pub fn record_price(&mut self, exchange: &str) {
        match exchange {
            "binance" => self.binance_updates += 1,
            "kraken"  => self.kraken_updates += 1,
            _ => {}
        }
    }

    pub fn record(&mut self, opp: &ArbitrageOpportunity) {
        self.total_opportunities += 1;
        if opp.is_stale { self.stale_opportunities += 1; }
        if opp.spread_pct > self.max_spread_pct { self.max_spread_pct = opp.spread_pct; }
        if opp.estimated_profit_pct > self.max_profit_pct { self.max_profit_pct = opp.estimated_profit_pct; }
        self.durations.push(opp.signal_duration_ms);
        self.profits.push(opp.estimated_profit_pct);
    }

    pub fn report(&self) {
        info!(
            "[stats] opportunities={} stale={} max_spread={:.4}% max_profit={:.4}% | updates: binance={} kraken={}",
            self.total_opportunities, self.stale_opportunities,
            self.max_spread_pct, self.max_profit_pct,
            self.binance_updates, self.kraken_updates,
        );

        if self.durations.is_empty() { return; }

        // Duration distribution
        let mut d = self.durations.clone();
        d.sort();
        let p50 = d[d.len() * 50 / 100];
        let p90 = d[d.len() * 90 / 100];
        let p99 = d[d.len().saturating_sub(1)];
        let tradeable = d.iter().filter(|&&x| x > 200).count();

        info!(
            "[duration] p50={}ms p90={}ms p99={}ms | tradeable(>200ms)={}/{} ({:.0}%)",
            p50, p90, p99,
            tradeable, d.len(),
            tradeable as f64 / d.len() as f64 * 100.0
        );

        // Duration buckets
        let b0 = d.iter().filter(|&&x| x < 50).count();
        let b1 = d.iter().filter(|&&x| x >= 50 && x < 200).count();
        let b2 = d.iter().filter(|&&x| x >= 200 && x < 500).count();
        let b3 = d.iter().filter(|&&x| x >= 500).count();
        info!(
            "[duration] <50ms={} 50-200ms={} 200-500ms={} >500ms={}",
            b0, b1, b2, b3
        );

        // Profit distribution среди прибыльных
        let profitable: Vec<f64> = self.profits.iter().copied().filter(|&p| p > 0.01).collect();
        if !profitable.is_empty() {
            let avg = profitable.iter().sum::<f64>() / profitable.len() as f64;
            let max = profitable.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            info!(
                "[profit] profitable={}/{} avg={:.4}% max={:.4}%",
                profitable.len(), self.profits.len(), avg, max
            );
        }
    }
}
