import sqlite3
import time
from pathlib import Path
from fastapi import APIRouter
from fastapi.responses import JSONResponse

from exchange.common import read_env

router = APIRouter()
DB = "arb_data.db"


def calc_trading_pnl() -> float:
    try:
        conn = sqlite3.connect(DB)
        tables = [r[0] for r in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table'"
        ).fetchall()]
        if "trades" not in tables:
            conn.close()
            return 0.0
        result = conn.execute(
            "SELECT SUM(profit_usd) FROM trades WHERE status='SUCCESS'"
        ).fetchone()
        conn.close()
        return round(result[0] or 0.0, 4)
    except Exception:
        return 0.0


def get_data() -> dict | None:
    if not Path(DB).exists():
        return None

    env = read_env()
    capital = float(env.get("CAPITAL_PER_EXCHANGE", 500))

    conn = sqlite3.connect(DB)
    conn.row_factory = sqlite3.Row

    opps = [dict(r) for r in conn.execute("""
        SELECT * FROM opportunities
        WHERE profit_pct >= 0.01
          AND buy_exchange  IN ('coinbase','binance','bitget')
          AND sell_exchange IN ('coinbase','binance','bitget')
          AND signal_score >= 4
        ORDER BY timestamp_ms DESC LIMIT 1000
    """).fetchall()]

    spreads = [dict(r) for r in conn.execute("""
        SELECT * FROM spreads ORDER BY timestamp_ms DESC LIMIT 100
    """).fetchall()]

    stats = dict(conn.execute(f"""
        SELECT COUNT(*) as total,
            AVG(profit_pct)            as avg_profit,
            MAX(profit_pct)            as max_profit,
            AVG(signal_duration_ms)    as avg_dur,
            AVG(normalized_spread_pct) as avg_norm,
            AVG(raw_spread_pct)        as avg_raw,
            SUM(MIN({capital}, max_size_usd) * profit_pct / 100.0) as real_profit_usd
        FROM (
            SELECT buy_exchange, sell_exchange, symbol,
                MAX(profit_pct)            as profit_pct,
                MAX(normalized_spread_pct) as normalized_spread_pct,
                MAX(raw_spread_pct)        as raw_spread_pct,
                MAX(signal_duration_ms)    as signal_duration_ms,
                MAX(max_size_usd)          as max_size_usd,
                signal_status
            FROM opportunities
            WHERE profit_pct >= 0.01 AND signal_status != 'weak'
              AND buy_exchange  IN ('coinbase','binance','bitget')
              AND sell_exchange IN ('coinbase','binance','bitget')
              AND signal_score >= 4
            GROUP BY buy_exchange, sell_exchange, symbol,
                     CAST(timestamp_ms / 30000 AS INTEGER)
        )
    """).fetchone())

    by_sym = [dict(r) for r in conn.execute("""
        SELECT symbol, COUNT(*) as cnt, AVG(profit_pct) as avg_profit,
            MAX(profit_pct) as max_profit,
            AVG(signal_duration_ms)/1000.0 as avg_dur_s
        FROM opportunities
        WHERE profit_pct >= 0.01 AND signal_status != 'weak'
          AND buy_exchange  IN ('coinbase','binance','bitget')
          AND sell_exchange IN ('coinbase','binance','bitget')
          AND signal_score >= 4
        GROUP BY symbol ORDER BY cnt DESC
    """).fetchall()]

    by_pair = [dict(r) for r in conn.execute("""
        SELECT buy_exchange || '→' || sell_exchange as pair, symbol,
            COUNT(*) as cnt, AVG(profit_pct) as avg_profit, MAX(profit_pct) as max_profit
        FROM opportunities
        WHERE profit_pct >= 0.01 AND signal_status != 'weak'
          AND buy_exchange  IN ('coinbase','binance','bitget')
          AND sell_exchange IN ('coinbase','binance','bitget')
          AND signal_score >= 4
        GROUP BY pair, symbol ORDER BY cnt DESC LIMIT 10
    """).fetchall()]

    dur_data = conn.execute("""
        SELECT signal_duration_ms FROM opportunities
        WHERE profit_pct >= 0.01 AND signal_status != 'weak'
          AND buy_exchange  IN ('coinbase','binance','bitget')
          AND sell_exchange IN ('coinbase','binance','bitget')
          AND signal_score >= 4
    """).fetchall()

    buckets = {"<1s": 0, "1-5s": 0, "5-15s": 0, ">15s": 0}
    for (d,) in dur_data:
        if d < 1000:       buckets["<1s"]   += 1
        elif d < 5000:     buckets["1-5s"]  += 1
        elif d < 15000:    buckets["5-15s"] += 1
        else:              buckets[">15s"]  += 1

    conn.close()
    actual_profit_usd = calc_trading_pnl()

    return {
        "opps": opps[:50],
        "spreads": spreads,
        "stats": stats,
        "capital": capital,
        "by_sym": by_sym,
        "by_pair": by_pair,
        "dur_buckets": buckets,
        "actual_profit_usd": actual_profit_usd,
    }


@router.get("/data")
async def api_data():
    data = get_data()
    return JSONResponse(content=data)
