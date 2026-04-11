use crate::models::ArbitrageOpportunity;
use tracing::info;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SimulatedTrade {
    pub opportunity: ArbitrageOpportunity,
    pub size_usd: f64,
    pub gross_profit_usd: f64,
    pub fees_usd: f64,
    pub net_profit_usd: f64,
}

pub struct Simulator {
    trade_size_usd: f64,
    trades: Vec<SimulatedTrade>,
    cumulative_pnl: f64,
}

impl Simulator {
    pub fn new(trade_size_usd: f64) -> Self {
        Self { trade_size_usd, trades: Vec::new(), cumulative_pnl: 0.0 }
    }

    pub fn record(&mut self, opp: &ArbitrageOpportunity) -> f64 {
        let gross_profit_usd = self.trade_size_usd * opp.spread_pct / 100.0;
        let fees_usd = self.trade_size_usd * (opp.spread_pct - opp.estimated_profit_pct) / 100.0;
        let net_profit_usd = self.trade_size_usd * opp.estimated_profit_pct / 100.0;

        self.cumulative_pnl += net_profit_usd;

        let trade = SimulatedTrade {
            opportunity: opp.clone(),
            size_usd: self.trade_size_usd,
            gross_profit_usd,
            fees_usd,
            net_profit_usd,
        };

        info!(
            "[simulator] trade #{} {} buy={} sell={} | net=${:.4} cumPnL=${:.4}{}",
            self.trades.len() + 1,
            opp.symbol,
            opp.buy_exchange, opp.sell_exchange,
            net_profit_usd, self.cumulative_pnl,
            if opp.is_stale { " [STALE]" } else { "" }
        );

        self.trades.push(trade);
        net_profit_usd
    }

    pub fn report(&self) {
        let total = self.trades.len();
        if total == 0 {
            info!("[simulator] no trades recorded yet");
            return;
        }
        let profitable = self.trades.iter().filter(|t| t.net_profit_usd > 0.0).count();
        let stale = self.trades.iter().filter(|t| t.opportunity.is_stale).count();
        let avg = self.cumulative_pnl / total as f64;
        let max = self.trades.iter().map(|t| t.net_profit_usd).fold(f64::NEG_INFINITY, f64::max);
        info!(
            "[simulator] REPORT | trades={} profitable={} stale={} | cumPnL=${:.4} avg=${:.4} max=${:.4}",
            total, profitable, stale, self.cumulative_pnl, avg, max
        );
    }
}
