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

use std::sync::Arc;
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

    // Читаем .env вручную
    if let Ok(content) = std::fs::read_to_string(".env") {
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() { continue; }
            if let Some(eq) = line.find('=') {
                let key = &line[..eq].trim();
                let val = &line[eq+1..].trim();
                if std::env::var(key).is_err() { std::env::set_var(key, val); }
            }
        }
    }

    // Читаем API ключи из .env (если есть)
    let coinbase_key    = std::env::var("COINBASE_API_KEY").unwrap_or_default();
    let coinbase_secret = std::env::var("COINBASE_API_SECRET").unwrap_or_default();
    let bitget_key      = std::env::var("BITGET_API_KEY").unwrap_or_default();
    let bitget_secret   = std::env::var("BITGET_API_SECRET").unwrap_or_default();
    let bitget_pass     = std::env::var("BITGET_PASSPHRASE").unwrap_or_default();
    let binance_key     = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let binance_secret  = std::env::var("BINANCE_API_SECRET").unwrap_or_default();
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

    let min_profit_pct: f64 = std::env::var("MIN_PROFIT_PCT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.15);

    info!("=== crypto-arb scanner starting (7 exchanges × 3 symbols: ETH SOL XRP) ===");
    info!("execution mode: {}", if dry_run { "DRY RUN 🔒" } else { "LIVE 🔴" });
    info!("max_trade_usd: ${:.0}  max_daily_loss: ${:.0}  min_profit: {:.2}%", max_trade_usd, max_daily_loss_usd, min_profit_pct);

    let mut opp_writer = match OpportunityWriter::new("opportunities.csv") {
        Ok(w) => { info!("📄 {}", w.path()); Some(w) }
        Err(e) => { error!("csv error: {e}"); None }
    };
    let mut spread_writer = match SpreadHistoryWriter::new("spreads.csv") {
        Ok(w) => { info!("📄 spreads.csv"); Some(w) }
        Err(e) => { error!("csv error: {e}"); None }
    };

    // Инициализируем SQLite — оборачиваем в Arc для шаринга с executor
    let database: Option<Arc<Database>> = match Database::new("arb_data.db") {
        Ok(db) => { info!("📊 database: arb_data.db"); Some(Arc::new(db)) }
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
        min_profit_pct,
        ..RiskConfig::default()
    });

    // Execution engine
    let exec_config = ExecutionConfig {
        dry_run,
        min_score: 4,
        max_trade_usd,
        skip_stale: true,
    };
    let mut executor = ArbitrageExecutor::new(
        exec_config,
        database.clone(), // Arc<Database> шарится с executor для записи liquidity_sales
        coinbase_key, coinbase_secret,
        bitget_key, bitget_secret, bitget_pass,
        binance_key, binance_secret,
        bybit_key, bybit_secret,
    );

    // Загружаем начальные балансы со всех бирж (крипта + стейблкоины)
    executor.sync_balances().await;

    // Покупаем недостающие активы на биржах (ETH, SOL, XRP)
    executor.inventory_optimize().await;

    let mut stats_timer = interval(Duration::from_secs(60));
    let mut daily_reset = interval(Duration::from_secs(86400));
    let mut price_log_counter: u64 = 0;
    let mut current_prices: std::collections::HashMap<(String, String), f64> = std::collections::HashMap::new();

    info!("listening for prices...");

    loop {
        tokio::select! {
            Some(price) = rx.recv() => {
                price_log_counter += 1;
                stats.record_price(&price.exchange);

                let mid = (price.bid + price.ask) / 2.0;
                current_prices.insert((price.exchange.clone(), price.symbol.clone()), mid);

                // Пишем цены в БД каждые 50 тиков
                if price_log_counter % 50 == 0 {
                    if let Some(db) = &database {
                        let _ = db.insert_price(&price);
                    }
                }

                if price_log_counter % 500 == 0 {
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

                    if opp.estimated_profit_pct >= crate::config::MIN_PROFIT_PCT_CSV {
                        if let Some(db) = &database {
                            if let Err(e) = db.insert_opportunity(&opp) {
                                error!("db insert opp: {e}");
                            }
                        }
                    }

                    match risk.check(&opp) {
                        RiskVerdict::Approved => {
                            if let Some(exec) = executor.try_execute(&opp).await {
                                let pnl = exec.actual_profit_usd;
                                risk.record_trade(pnl);

                                if let Some(db) = &database {
                                    let status = if exec.success { "SUCCESS" }
                                        else if exec.actual_profit_usd == 0.0 { "BUY_FAILED" }
                                        else { "PARTIAL" };
                                    let buy_cost = exec.buy_order.quantity * opp.buy_price * (1.0 + 0.0005);
                                    let sell_recv = if exec.success {
                                        exec.sell_order.quantity * opp.sell_price * (1.0 - 0.0010)
                                    } else { 0.0 };
                                    if let Err(e) = db.insert_trade(
                                        opp.timestamp_ms,
                                        &opp.buy_exchange, &opp.sell_exchange, &opp.symbol,
                                        exec.buy_order.quantity, opp.buy_price, opp.sell_price,
                                        buy_cost, sell_recv,
                                        exec.actual_profit_usd, opp.estimated_profit_pct,
                                        exec.buy_order.quantity * opp.buy_price,
                                        exec.latency_ms, status,
                                    ) {
                                        error!("db insert trade: {e}");
                                    }
                                }
                            }
                            simulator.record(&opp);
                        }
                        RiskVerdict::Rejected => {
                            tracing::debug!("[risk] rejected: profit={:.4}% score={} stale={}",
                                opp.estimated_profit_pct, opp.signal_score, opp.is_stale);
                        }
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
                executor.try_exit_stuck().await;
                executor.try_liquidity_recovery(&current_prices).await;
                if executor.inventory.needs_sync() && !dry_run {
                    executor.sync_balances().await;
                }
            }

            _ = daily_reset.tick() => {
                info!("[risk] resetting daily counters");
                risk.reset_daily();
            }
        }
    }
}
