# Crypto Arbitrage Bot — Project Documentation

## What This Is

A **cross-exchange cryptocurrency arbitrage bot** written in Rust. It monitors real-time prices across 7 exchanges via WebSocket, detects price differences (spreads), and executes buy-low/sell-high trades across exchanges.

**Owner:** Alexandr (Israel, timezone Asia/Jerusalem)
**Server:** VPS at `109.199.114.215`, Ubuntu 24, runs via systemd
**Capital:** ~$600 across 4 exchanges (~$150 each)
**Status:** Live trading, profitable (~$0.15-0.85/day)

---

## Architecture Overview

```
WebSocket Price Feeds (7 exchanges)
         │
         ▼
   Opportunity Detector ──→ SQLite DB (arb_data.db)
   (spread detection,        │
    fee calculation,         ▼
    signal scoring)     Dashboard (Python/FastAPI)
         │
         ▼
     Risk Engine (min_profit, daily loss limit)
         │
         ▼
    Executor ──→ Buy on Exchange A
         │  ──→ Sell on Exchange B
         ▼
    Inventory Manager (balance tracking per exchange)
         │
         ▼
    Inventory Optimizer (auto-rebalance at startup)
```

---

## Exchanges

| Exchange | Role | API | Fee | Stablecoin |
|----------|------|-----|-----|------------|
| **Coinbase** | Trade (buy/sell) | REST via Python subprocess (JWT) | 0.05% | USDC |
| **Binance** | Trade (buy/sell) | REST (HMAC-SHA256) | 0.10% | USDT |
| **Bitget** | Trade (buy/sell) | REST (HMAC-SHA256 base64) | 0.10% | USDT |
| **Bybit** | Trade (buy/sell) | REST (HMAC-SHA256) | 0.10% | USDT |
| Kraken | Price feed only | WebSocket | 0.26% | — |
| OKX | Price feed only | WebSocket | 0.10% | — |
| Gate.io | Price feed only | WebSocket | 0.20% | — |

**Traded symbols:** ETH, SOL, XRP
**All 7 exchanges** provide price data via WebSocket.
**4 exchanges** (Coinbase, Binance, Bitget, Bybit) have API keys for execution.

---

## File Structure

```
crypto-arb_v1/
├── Cargo.toml                    # Rust dependencies
├── .env                          # API keys & config (NOT in repo)
├── arb_data.db                   # SQLite database
├── bot.log                       # Main log file (can be 100MB+)
├── coinbase_key.pem              # Coinbase API private key
├── start_balance.json            # Initial balance snapshot
│
├── src/
│   ├── main.rs                   # Entry point, event loop, .env reader
│   ├── config.rs                 # Constants: fees, symbols, thresholds
│   ├── models.rs                 # ArbitrageOpportunity, TradeResult structs
│   │
│   ├── market_data/              # WebSocket price feeds (7 exchanges)
│   │   ├── mod.rs
│   │   ├── binance.rs            # Binance WS bookTicker
│   │   ├── bitget.rs             # Bitget WS ticker
│   │   ├── bybit.rs              # Bybit WS ticker
│   │   ├── coinbase.rs           # Coinbase WS ticker
│   │   ├── gateio.rs             # Gate.io WS ticker
│   │   ├── kraken.rs             # Kraken WS ticker
│   │   ├── okx.rs                # OKX WS ticker
│   │   └── usdt_usd.rs           # USDT/USD rate tracker
│   │
│   ├── opportunity_detector/     # Spread detection & signal scoring
│   │   └── mod.rs                # detect_for_symbol(), calc_score()
│   │
│   ├── risk_engine/              # Trade approval
│   │   └── mod.rs                # min_profit, daily loss, max trades
│   │
│   ├── execution/                # Order execution
│   │   ├── mod.rs
│   │   ├── executor.rs           # Main executor: try_execute(), inventory_optimize()
│   │   ├── coinbase.rs           # Coinbase API (via python3 subprocess)
│   │   ├── binance.rs            # Binance API (reqwest + HMAC)
│   │   ├── bitget.rs             # Bitget API (reqwest + HMAC base64)
│   │   ├── bybit.rs              # Bybit API (reqwest + HMAC)
│   │   └── inventory.rs          # Balance tracking per (exchange, asset)
│   │
│   ├── db/                       # SQLite: prices, spreads, opportunities, trades
│   │   └── mod.rs
│   │
│   ├── monitoring/               # Stats logging
│   │   └── mod.rs
│   │
│   ├── simulator/                # Paper trading simulator
│   │   └── mod.rs
│   │
│   ├── storage/                  # CSV writers (opportunities.csv, spreads.csv)
│   │   └── mod.rs
│   │
│   └── price_engine/             # (empty, placeholder)
│       └── mod.rs
│
└── dashboard/                    # Web dashboard (FastAPI)
    ├── server.py                 # FastAPI app, routes
    ├── requirements.txt          # fastapi, uvicorn, jinja2
    ├── api/
    │   ├── balances.py           # /api/balances endpoint
    │   ├── data.py               # /api/data — main dashboard data
    │   ├── trades.py             # /api/trades — trade history
    │   ├── orders.py             # /api/orders — real exchange orders
    │   └── logs.py               # /api/logs — bot.log reader
    ├── exchange/
    │   ├── balances.py           # Balance fetchers per exchange
    │   ├── orders.py             # Order fetchers per exchange
    │   └── common.py             # Shared utils (read_env, HMAC)
    ├── templates/
    │   ├── base.html             # Base layout with nav
    │   ├── index.html            # Main dashboard
    │   ├── trades.html           # Trade diary
    │   └── logs.html             # Log viewer
    └── static/
        ├── css/main.css          # Styles
        └── js/
            ├── dashboard.js      # Dashboard logic
            ├── trades.js         # Trades page logic
            └── logs.js           # Logs page logic
```

---

## Key Configuration (.env)

```env
# Execution mode
DRY_RUN=false
MAX_TRADE_USD=50
MAX_DAILY_LOSS_USD=10
MIN_PROFIT_PCT=0.01

# Coinbase
COINBASE_API_KEY=organizations/xxx
COINBASE_API_SECRET=coinbase_key.pem

# Binance
BINANCE_API_KEY=xxx
BINANCE_API_SECRET=xxx

# Bitget
BITGET_API_KEY=xxx
BITGET_API_SECRET=xxx
BITGET_PASSPHRASE=xxx

# Bybit
BYBIT_API_KEY=xxx
BYBIT_API_SECRET=xxx
```

---

## How the Bot Works (Execution Flow)

### 1. Price Collection
All 7 exchanges stream bid/ask prices via WebSocket into an mpsc channel.

### 2. Opportunity Detection (`opportunity_detector/mod.rs`)
For each price update, compares all exchange pairs:
```
estimated_profit_pct = spread_pct - buy_fee - sell_fee
```
**Profit is ALREADY net of fees.** Do NOT add another fee filter.

Signal scoring (0-10): profit weight + duration weight + tick count - stale penalty.

### 3. Risk Check (`risk_engine/mod.rs`)
Approves if: `profit >= min_profit_pct` AND `daily_trades < max` AND `daily_pnl > -max_loss`

### 4. Execution (`executor.rs :: try_execute()`)
- Filters: score >= 6, not stale, exchange is "ours" (coinbase/binance/bitget/bybit)
- Adapts trade size to available balances (stablecoin for buy, crypto for sell)
- **Pre-flight check**: for Coinbase buys, calls API to verify real balance
- **Sequential**: BUY first, then SELL (if buy fails → no sell, no damage)
- **Cooldown**: 30 seconds per (buy_exchange, sell_exchange, symbol) pair
- On sell failure: tracks as "stuck position" for auto-exit

### 5. Inventory Management
- `sync_balances()`: fetches real balances from all exchanges every 60s
- `check_rebalance()`: logs warnings if exchanges are imbalanced
- `inventory_optimize()`: at startup, buys missing assets to reach 50% stablecoin / 50% crypto split
- `try_exit_stuck()`: periodically tries to sell crypto stuck from failed sells

---

## Systemd Services

```bash
# Bot
systemctl status crypto-arb
systemctl restart crypto-arb
# Config: /etc/systemd/system/crypto-arb.service
# Logs to: /Bot-scanner/crypto-arb_v0.2/bot.log

# Dashboard
systemctl status crypto-dash
systemctl restart crypto-dash
# Config: /etc/systemd/system/crypto-dash.service
# Port: 8765
```

---

## Database Schema (arb_data.db)

**prices** — sampled every 50 ticks
**spreads** — best bid/ask snapshots every 30s
**opportunities** — all opportunities with profit >= 0.01%
**trades** — executed trades with status (SUCCESS, BUY_FAILED, PARTIAL)

---

## Important Quirks & Gotchas

### ⚠️ DO NOT raise min_profit_pct above 0.01%
`estimated_profit_pct` already has fees subtracted. Setting 0.15% means requiring 0.30%+ raw spread (fees counted twice). This was a bug we fixed.

### ⚠️ Coinbase API is slow
Balance check takes ~500ms (Python subprocess). Pre-flight check adds latency but prevents INSUFFICIENT_FUND errors.

### ⚠️ Bitget market buy uses USDT amount
For market BUY on Bitget, `size` field = USDT amount, NOT coin quantity. Use `buy_by_quote()` method.

### ⚠️ Bybit market buy needs `marketUnit: "quoteCoin"`
Without this, `qty` is interpreted as base coin quantity.

### ⚠️ bot.log has ANSI escape codes
Dashboard log reader must strip `\x1b[...m` codes before parsing timestamps.

### ⚠️ Never suggest restarting bot for debugging
Alexandr has explicitly stated this causes log loss and is frustrating. Do not recommend restarts unless absolutely necessary.

### ⚠️ bot.log can be 100MB+
Log reader must use `tail` approach, never read entire file. Dashboard currently reads last 2000 lines.

---

## Current Performance

- **Win rate:** ~70% (BUY_FAILED counts as loss but is actually just "no funds" skip)
- **Avg profit/trade:** $0.01-0.03
- **Best single trade:** $0.73 (ETH arbitrage)
- **Daily profit:** $0.14-0.85 depending on volatility
- **Monthly estimate:** $5-25 at current capital

---

## Known Issues / TODO

1. **BUY_FAILED clutters win rate** — pre-flight check should cover all exchanges, not just Coinbase. BUY_FAILED should show "SKIPPED: no funds" instead.

2. **Dashboard is slow** — old single-file Python HTTP server being replaced with FastAPI + separate HTML/JS/CSS files. The new `dashboard/` folder has this structure but may need finishing.

3. **Inventory optimizer runs only at startup** — could run periodically to rebalance after drift.

4. **No OKX execution** — OKX has good signals but no API keys configured yet. Adding would give more trading pairs.

5. **Simulator trades count is inflated** — simulator trades on every signal tick, not just once per window. CumPnL numbers ($49K+) are unrealistic.

---

## How to Deploy Changes

```bash
# 1. Copy changed .rs files to server
scp file.rs root@109.199.114.215:/Bot-scanner/crypto-arb_v0.2/src/path/

# 2. Build
cd /Bot-scanner/crypto-arb_v0.2
cargo build --release

# 3. Restart bot (preserves bot.log)
systemctl restart crypto-arb

# 4. For dashboard changes
cp dashboard.py /Bot-scanner/crypto-arb_v0.2/dashboard.py
systemctl restart crypto-dash

# 5. Check logs
tail -50 /Bot-scanner/crypto-arb_v0.2/bot.log
journalctl -u crypto-arb --no-pager -n 20
```

---

## For LLM Assistants

When helping with this project:

1. **Always read the relevant source file** before suggesting changes
2. **Profit already includes fees** — don't add fee filtering on top
3. **Test Python syntax** with `py_compile` before sharing
4. **Package as zip** when multiple files change — single .rs files can corrupt on download
5. **bot.log has ANSI codes** — account for `\x1b[...m` in any log parsing
6. **Coinbase uses Python subprocess** for API calls (JWT auth), other exchanges use reqwest
7. **Server path:** `/Bot-scanner/crypto-arb_v0.2/` (despite v1 in archive name)
8. **Dashboard port:** 8765
9. **All times in bot.log are UTC** — dashboard converts to Israel time (UTC+3)
10. **Do NOT recommend restarting the bot** as a debugging step
