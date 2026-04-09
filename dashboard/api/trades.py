import sqlite3
from pathlib import Path
from fastapi import APIRouter
from fastapi.responses import JSONResponse

router = APIRouter()
DB = "arb_data.db"


def get_trades_data() -> dict:
    if not Path(DB).exists():
        return {"trades": [], "summary": {}, "liq_summary": {}, "liq_sales": []}
    try:
        conn = sqlite3.connect(DB)
        conn.row_factory = sqlite3.Row
        tables = [
            r[0]
            for r in conn.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
        ]

        # ── Арбитражные сделки ────────────────────────────────────────────
        if "trades" not in tables:
            conn.close()
            return {"trades": [], "summary": {}, "liq_summary": {}, "liq_sales": []}

        trades = [
            dict(r)
            for r in conn.execute(
                "SELECT * FROM trades ORDER BY timestamp_ms DESC LIMIT 200"
            ).fetchall()
        ]
        summary = dict(
            conn.execute("""
                SELECT
                    COUNT(*) as total,
                    SUM(CASE WHEN status='SUCCESS'    THEN 1 ELSE 0 END) as success,
                    SUM(CASE WHEN status='BUY_FAILED' THEN 1 ELSE 0 END) as buy_failed,
                    SUM(CASE WHEN status NOT IN ('SUCCESS','BUY_FAILED') THEN 1 ELSE 0 END) as other_failed,
                    SUM(CASE WHEN status='SUCCESS' THEN profit_usd      ELSE 0 END) as total_profit,
                    SUM(CASE WHEN status='SUCCESS' THEN trade_size_usd  ELSE 0 END) as total_volume,
                    AVG(CASE WHEN status='SUCCESS' THEN profit_usd END)  as avg_profit,
                    AVG(CASE WHEN status='SUCCESS' THEN latency_ms END)  as avg_latency
                FROM trades
            """).fetchone()
        )
        by_pair = [
            dict(r)
            for r in conn.execute("""
                SELECT
                    buy_exchange || ' -> ' || sell_exchange as pair,
                    COUNT(*) as total,
                    SUM(CASE WHEN status='SUCCESS' THEN 1          ELSE 0 END) as success,
                    SUM(CASE WHEN status='SUCCESS' THEN profit_usd ELSE 0 END) as profit,
                    SUM(CASE WHEN status='SUCCESS' THEN trade_size_usd ELSE 0 END) as volume
                FROM trades
                GROUP BY pair
                ORDER BY profit DESC
            """).fetchall()
        ]

        # ── Продажи ликвидности (НЕ арбитраж) ────────────────────────────
        liq_summary = {}
        liq_sales = []
        liq_by_exchange = []

        if "liquidity_sales" in tables:
            liq_summary = dict(
                conn.execute("""
                    SELECT
                        COUNT(*)                                                             AS total_sales,
                        SUM(CASE WHEN status='SUCCESS' THEN 1 ELSE 0 END)                   AS success_sales,
                        SUM(CASE WHEN status='FAILED'  THEN 1 ELSE 0 END)                   AS failed_sales,
                        SUM(CASE WHEN status='SUCCESS' THEN pnl_usd ELSE 0 END)             AS total_pnl,
                        AVG(CASE WHEN status='SUCCESS' THEN pnl_usd END)                    AS avg_pnl,
                        SUM(CASE WHEN status='SUCCESS' AND pnl_usd > 0  THEN 1 ELSE 0 END) AS pnl_positive,
                        SUM(CASE WHEN status='SUCCESS' AND pnl_usd = 0  THEN 1 ELSE 0 END) AS pnl_zero,
                        SUM(CASE WHEN status='SUCCESS' AND pnl_usd < 0  THEN 1 ELSE 0 END) AS pnl_negative,
                        SUM(CASE WHEN sale_type='LIQUIDITY_RECOVERY' AND status='SUCCESS' THEN 1 ELSE 0 END) AS liq_recovery_cnt,
                        SUM(CASE WHEN sale_type='STUCK_EXIT'         AND status='SUCCESS' THEN 1 ELSE 0 END) AS stuck_exit_cnt
                    FROM liquidity_sales
                """).fetchone()
            )

            liq_sales = [
                dict(r)
                for r in conn.execute("""
                    SELECT
                        timestamp_ms, sale_type, exchange, symbol, qty,
                        buy_price, sell_price,
                        buy_cost_usd, sell_recv_usd,
                        pnl_usd, pnl_pct, status
                    FROM liquidity_sales
                    ORDER BY timestamp_ms DESC
                    LIMIT 100
                """).fetchall()
            ]

            liq_by_exchange = [
                dict(r)
                for r in conn.execute("""
                    SELECT
                        exchange,
                        symbol,
                        sale_type,
                        COUNT(*)                                                         AS cnt,
                        SUM(CASE WHEN status='SUCCESS' THEN pnl_usd ELSE 0 END)         AS total_pnl,
                        AVG(CASE WHEN status='SUCCESS' THEN pnl_usd END)                AS avg_pnl,
                        SUM(CASE WHEN status='SUCCESS' AND pnl_usd > 0 THEN 1 ELSE 0 END) AS profit_cnt,
                        SUM(CASE WHEN status='SUCCESS' AND pnl_usd <= 0 THEN 1 ELSE 0 END) AS zero_or_loss_cnt
                    FROM liquidity_sales
                    WHERE status = 'SUCCESS'
                    GROUP BY exchange, symbol, sale_type
                    ORDER BY exchange, symbol
                """).fetchall()
            ]

        conn.close()
        return {
            "trades": trades,
            "summary": summary,
            "by_pair": by_pair,
            "liq_summary": liq_summary,
            "liq_sales": liq_sales,
            "liq_by_exchange": liq_by_exchange,
        }
    except Exception as e:
        return {"trades": [], "summary": {}, "liq_summary": {}, "liq_sales": [], "error": str(e)}


@router.get("/trades_data")
async def api_trades_data():
    return JSONResponse(content=get_trades_data())
