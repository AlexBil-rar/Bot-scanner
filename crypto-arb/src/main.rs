mod config;
mod db;
mod execution;
mod market_data;
mod models;
mod monitoring;
mod opportunity_detector;
mod risk_engine;
mod simulator;
mod storage;

use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{error, info};

use db::Database;
use execution::executor::{ArbitrageExecutor, ExecutionConfig};
use monitoring::Stats;
use opportunity_detector::OpportunityDetector;
use risk_engine::{RiskConfig, RiskEngine, RiskVerdict};
use simulator::Simulator;
use storage::{OpportunityWriter, SpreadHistoryWriter};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    dotenv::dotenv().ok();

    // Читаем API ключи из .env (если есть)
    let coinbase_key    = std::env::var("COINBASE_API_KEY").unwrap_or_default();
    let coinbase_secret = std::env::var("COINBASE_API_SECRET").unwrap_or_default();
    let bybit_key       = std::env::var("BYBIT_API_KEY").unwrap_or_default();
    let bybit_secret    = std::env::var("BYBIT_API_SECRET").unwrap_or_default();

    let dry_run = std::env::var("DRY_RUN")
        .map(|v| v != "false")
        .unwrap_or(true);

    let max_trade_usd: f64 = std::env::var("MAX_TRADE_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500.0);

    let max_daily_loss_usd: f64 = std::env::var("MAX_DAILY_LOSS_USD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50.0);

    info!("=== crypto-arb scanner starting (7 exchanges × 5 symbols) ===");
    info!("execution mode: {}", if dry_run { "DRY RUN 🔒" } else { "LIVE 🔴" });
    info!("max_trade_usd: ${:.0}  max_daily_loss: ${:.0}", max_trade_usd, max_daily_loss_usd);

    let mut opp_writer = match OpportunityWriter::new("opportunities.csv") {
        Ok(w) => { info!("📄 {}", w.path()); Some(w) }
        Err(e) => { error!("csv error: {e}"); None }
    };
    let mut spread_writer = match SpreadHistoryWriter::new("spreads.csv") {
        Ok(w) => { info!("📄 spreads.csv"); Some(w) }
        Err(e) => { error!("csv error: {e}"); None }
    };

    // Инициализируем SQLite
    let database = match Database::new("arb_data.db") {
        Ok(db) => { info!("📊 database: arb_data.db"); Some(db) }
        Err(e) => { error!("db error: {e}"); None }
    };

    // Запускаем трекер USDT/USD курса
    let usdt_rate = market_data::usdt_usd::new_rate();
    {
        let rate = usdt_rate.clone();
        tokio::spawn(async move { market_data::usdt_usd::stream(rate).await; });
    }

    let (tx, mut rx) = mpsc::channel::<models::MarketPrice>(4096);

    tokio::spawn({ let t = tx.clone(); async move { market_data::binance::stream(t).await; } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::kraken::stream(t).await;  } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::okx::stream(t).await;     } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::bybit::stream(t).await;   } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::gateio::stream(t).await;  } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::bitget::stream(t).await;  } });
    tokio::spawn({ let t = tx.clone(); async move { market_data::coinbase::stream(t).await;} });
    drop(tx);

    let mut detector  = OpportunityDetector::new(usdt_rate.clone());
    let mut stats     = Stats::new();
    let mut simulator = Simulator::new(1000.0);
    let mut risk      = RiskEngine::new(RiskConfig {
        max_trade_size_usd: max_trade_usd,
        max_daily_loss_usd,
        ..RiskConfig::default()
    });

    // Execution engine — dry_run по умолчанию
    let exec_config = ExecutionConfig {
        dry_run,
        min_score: 6,
        max_trade_usd,
        skip_stale: true,
    };
    let mut executor = ArbitrageExecutor::new(
        exec_config,
        coinbase_key, coinbase_secret,
        bybit_key, bybit_secret,
    );

    let mut stats_timer = interval(Duration::from_secs(60));
    let mut daily_reset = interval(Duration::from_secs(86400));
    let mut price_log_counter: u64 = 0;

    info!("listening for prices...");

    loop {
        tokio::select! {
            Some(price) = rx.recv() => {
                price_log_counter += 1;
                stats.record_price(&price.exchange);

                // Пишем цены в БД каждые 50 тиков
                if price_log_counter % 50 == 0 {
                    if let Some(db) = &database {
                        let _ = db.insert_price(&price);
                    }
                }

                let slow = matches!(price.exchange.as_str(), "kraken" | "gateio" | "bitget" | "coinbase");
                if slow || price_log_counter % 20 == 0 {
                    info!(
                        "[price] {:<9} {:<4} bid={:.4} ask={:.4}",
                        price.exchange, price.symbol, price.bid, price.ask
                    );
                }

                let (opportunities, spread_snapshot) = detector.update(price);

                if let Some(snap) = &spread_snapshot {
                    if let Some(w) = &mut spread_writer {
                        if let Err(e) = w.write(snap.timestamp_ms, &snap.symbol,
                            &snap.best_bid_exchange, snap.best_bid,
                            &snap.best_ask_exchange, snap.best_ask) {
                            error!("spread csv: {e}");
                        }
                    }
                    if let Some(db) = &database {
                        let spread = (snap.best_bid - snap.best_ask) / snap.best_ask * 100.0;
                        if let Err(e) = db.insert_spread(
                            snap.timestamp_ms, &snap.symbol,
                            &snap.best_bid_exchange, snap.best_bid,
                            &snap.best_ask_exchange, snap.best_ask, spread,
                        ) { error!("db insert spread: {e}"); }
                    }
                }

                for opp in opportunities {
                    stats.record(&opp);

                    if let Some(w) = &mut opp_writer {
                        match w.write_if_profitable(&opp) {
                            Ok(true) => info!("💾 [CSV] #{} {}", w.count(), w.path()),
                            Ok(false) => {}
                            Err(e) => error!("csv: {e}"),
                        }
                    }

                    // Пишем ВСЕ profitable opportunities в БД
                    if opp.estimated_profit_pct >= crate::config::MIN_PROFIT_PCT_CSV {
                        if let Some(db) = &database {
                            if let Err(e) = db.insert_opportunity(&opp) {
                                error!("db insert opp: {e}");
                            }
                        }
                    }

                    // Execution (dry run пока нет ключей)
                    executor.try_execute(&opp).await;

                    match risk.check(&opp) {
                        RiskVerdict::Approved => {
                            let pnl = simulator.record(&opp);
                            risk.record_trade(pnl);
                        }
                        RiskVerdict::Rejected => {}
                    }
                }
            }

            _ = stats_timer.tick() => {
                stats.report();
                simulator.report();
                executor.report();
                if let Some(w) = &opp_writer {
                    info!("[csv] profitable written: {}", w.count());
                }
            }

            _ = daily_reset.tick() => {
                info!("[risk] resetting daily counters");
                risk.reset_daily();
            }
        }
    }
}
