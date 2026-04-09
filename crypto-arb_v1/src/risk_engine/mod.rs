use tracing::warn;
use crate::models::ArbitrageOpportunity;

pub struct RiskConfig {
    pub max_trade_size_usd: f64,
    pub max_daily_loss_usd: f64,
    pub max_daily_trades: usize,
    pub min_profit_pct: f64,
    pub reject_stale: bool,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_trade_size_usd: 1000.0,
            max_daily_loss_usd: 50.0,
            max_daily_trades: 100,
            min_profit_pct: 0.15,
            reject_stale: true,
        }
    }
}

pub struct RiskEngine {
    config: RiskConfig,
    daily_trades: usize,
    daily_pnl: f64,
}

#[derive(Debug)]
pub enum RiskVerdict {
    Approved,
    Rejected,
}

impl RiskEngine {
    pub fn new(config: RiskConfig) -> Self {
        Self { config, daily_trades: 0, daily_pnl: 0.0 }
    }

    pub fn check(&self, opp: &ArbitrageOpportunity) -> RiskVerdict {
        if self.config.reject_stale && opp.is_stale {
            return RiskVerdict::Rejected;
        }
        if opp.estimated_profit_pct < self.config.min_profit_pct {
            return RiskVerdict::Rejected;
        }
        if self.daily_trades >= self.config.max_daily_trades {
            return RiskVerdict::Rejected;
        }
        if self.daily_pnl <= -self.config.max_daily_loss_usd {
            return RiskVerdict::Rejected;
        }
        RiskVerdict::Approved
    }

    pub fn record_trade(&mut self, pnl_usd: f64) {
        self.daily_trades += 1;
        self.daily_pnl += pnl_usd;
        warn!("[risk] daily_trades={} daily_pnl=${:.4}", self.daily_trades, self.daily_pnl);
    }

    pub fn reset_daily(&mut self) {
        self.daily_trades = 0;
        self.daily_pnl = 0.0;
    }

    #[allow(dead_code)]
    pub fn max_trade_size(&self) -> f64 {
        self.config.max_trade_size_usd
    }
}
