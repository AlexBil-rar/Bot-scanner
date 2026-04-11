#!/usr/bin/env python3
"""
Арбитраж дашборд — читает arb_data.db и показывает реальные балансы
Запуск: python3 dashboard.py
"""
import re
import sqlite3, json, os, http.server
import hmac, hashlib, base64, time, urllib.request, urllib.parse
from pathlib import Path

DB = "arb_data.db"
PORT = 8765

# ---------------------------------------------------------------------------
# Кэш балансов — обновляем раз в минуту
# ---------------------------------------------------------------------------
_balance_cache = {"ts": 0, "data": None}

ROOT_DIR = Path(__file__).resolve().parent

def read_env():
    env = {}
    for f in [ROOT_DIR / '.env', ROOT_DIR / '.env.example']:
        p = Path(f)
        if p.exists():
            for line in p.read_text().splitlines():
                line = line.strip()
                if '=' in line and not line.startswith('#'):
                    k, v = line.split('=', 1)
                    env[k.strip()] = v.strip()
            break
    return env

# ---------------------------------------------------------------------------
# Exchange balance fetchers
# ---------------------------------------------------------------------------

def get_coinbase_balance(api_key, api_secret):
    try:
        from coinbase.rest import RESTClient
        if api_secret.startswith('/') and Path(api_secret).exists():
            api_secret = Path(api_secret).read_text().strip()
        else:
            api_secret = api_secret.replace('\\n', '\n')
        client = RESTClient(api_key=api_key, api_secret=api_secret)
        accounts = client.get_accounts()
        total_usdc = 0.0
        crypto_holdings = {}
        for acc in accounts.accounts:
            val = float(acc.available_balance['value'] or 0)
            if val <= 0: continue
            if acc.currency in ('USD', 'USDC'):
                total_usdc += val
            elif acc.currency in ('XRP', 'BTC', 'ETH', 'SOL', 'BNB'):
                crypto_holdings[acc.currency] = val
        if crypto_holdings:
            try:
                products = client.get_best_bid_ask(product_ids=[f"{c}-USDC" for c in crypto_holdings])
                prices = {}
                for p in products.pricebooks:
                    symbol = p.product_id.split('-')[0]
                    if p.bids:
                        prices[symbol] = float(p.bids[0].price)
                for symbol, qty in crypto_holdings.items():
                    if symbol in prices:
                        total_usdc += qty * prices[symbol]
            except:
                pass
        return round(total_usdc, 2)
    except Exception:
        return None

def get_binance_balance(api_key, api_secret):
    try:
        timestamp = int(time.time() * 1000)
        params = f"timestamp={timestamp}&recvWindow=5000"
        signature = hmac.new(api_secret.encode(), params.encode(), hashlib.sha256).hexdigest()
        url = f"https://api.binance.com/api/v3/account?{params}&signature={signature}"
        req = urllib.request.Request(url, headers={"X-MBX-APIKEY": api_key})
        with urllib.request.urlopen(req, timeout=5) as r:
            data = json.loads(r.read())
        prices_url = "https://api.binance.com/api/v3/ticker/price"
        with urllib.request.urlopen(prices_url, timeout=5) as r:
            prices_data = json.loads(r.read())
        prices = {p["symbol"]: float(p["price"]) for p in prices_data}
        total = 0.0
        for bal in data.get("balances", []):
            asset = bal["asset"]
            free = float(bal["free"])
            if free <= 0: continue
            if asset in ("USDT", "BUSD"):
                total += free
            elif asset + "USDT" in prices:
                total += free * prices[asset + "USDT"]
        return round(total, 2)
    except Exception:
        return None

def get_bitget_balance(api_key, api_secret, passphrase):
    try:
        timestamp = str(int(time.time() * 1000))
        path = "/api/v2/spot/account/assets"
        message = timestamp + "GET" + path
        signature = base64.b64encode(
            hmac.new(api_secret.encode(), message.encode(), hashlib.sha256).digest()
        ).decode()
        req = urllib.request.Request(
            f"https://api.bitget.com{path}",
            headers={"ACCESS-KEY": api_key, "ACCESS-SIGN": signature,
                     "ACCESS-TIMESTAMP": timestamp, "ACCESS-PASSPHRASE": passphrase,
                     "Content-Type": "application/json"}
        )
        with urllib.request.urlopen(req, timeout=5) as r:
            data = json.loads(r.read())
        prices_url = "https://api.bitget.com/api/v2/spot/market/tickers"
        with urllib.request.urlopen(prices_url, timeout=5) as r:
            prices_data = json.loads(r.read())
        prices = {p["symbol"]: float(p["lastPr"]) for p in prices_data.get("data", [])}
        total = 0.0
        for asset in data.get("data", []):
            coin = asset.get("coin", "")
            available = float(asset.get("available", 0) or 0)
            if available <= 0: continue
            if coin in ("USDT", "USDC"):
                total += available
            elif coin + "USDT" in prices:
                total += available * prices[coin + "USDT"]
        return round(total, 2)
    except Exception:
        return None

def get_bybit_balance(api_key, api_secret):
    try:
        timestamp = str(int(time.time() * 1000))
        recv_window = "5000"
        params = "accountType=UNIFIED"
        sign_str = timestamp + api_key + recv_window + params
        signature = hmac.new(api_secret.encode(), sign_str.encode(), hashlib.sha256).hexdigest()
        url = f"https://api.bybit.com/v5/account/wallet-balance?{params}"
        req = urllib.request.Request(url, headers={
            "X-BAPI-API-KEY": api_key,
            "X-BAPI-SIGN": signature,
            "X-BAPI-TIMESTAMP": timestamp,
            "X-BAPI-RECV-WINDOW": recv_window,
        })
        with urllib.request.urlopen(req, timeout=5) as r:
            data = json.loads(r.read())
        if data.get("retCode") != 0:
            return None
        total = 0.0
        for account in data.get("result", {}).get("list", []):
            for coin in account.get("coin", []):
                total += float(coin.get("usdValue", 0) or 0)
        return round(total, 2)
    except Exception:
        return None

# ---------------------------------------------------------------------------
# Real orders from exchanges (since Apr 1 2026)
# ---------------------------------------------------------------------------

SYMBOLS = ["XRPUSDT", "SOLUSDT", "ETHUSDT", "BTCUSDT"]
START_TS = int(time.mktime(time.strptime("2026-04-01", "%Y-%m-%d"))) * 1000

def get_real_orders():
    orders = []
    env = read_env()

    # Binance
    try:
        api_key = env.get("BINANCE_API_KEY", "")
        api_secret = env.get("BINANCE_API_SECRET", "")
        ts = int(time.time() * 1000)
        for symbol in SYMBOLS:
            params = f"symbol={symbol}&startTime={START_TS}&limit=50&timestamp={ts}&recvWindow=5000"
            sig = hmac.new(api_secret.encode(), params.encode(), hashlib.sha256).hexdigest()
            url = f"https://api.binance.com/api/v3/allOrders?{params}&signature={sig}"
            req = urllib.request.Request(url, headers={"X-MBX-APIKEY": api_key})
            with urllib.request.urlopen(req, timeout=5) as r:
                data = json.loads(r.read())
            for o in data:
                if o.get("status") == "FILLED":
                    exec_qty = float(o["executedQty"])
                    price = (float(o["price"]) if float(o["price"]) > 0
                             else float(o.get("cummulativeQuoteQty", 0)) / max(exec_qty, 1e-9))
                    orders.append({
                        "exchange": "binance",
                        "symbol": symbol.replace("USDT", ""),
                        "side": o["side"],
                        "qty": exec_qty,
                        "price": price,
                        "total": float(o.get("cummulativeQuoteQty", 0)),
                        "time": int(o["time"]),
                        "status": "FILLED",
                    })
    except Exception:
        pass

    # Bitget
    try:
        api_key = env.get("BITGET_API_KEY", "")
        api_secret = env.get("BITGET_API_SECRET", "")
        passphrase = env.get("BITGET_PASSPHRASE", "")
        for symbol in SYMBOLS:
            timestamp = str(int(time.time() * 1000))
            path = f"/api/v2/spot/trade/history-orders?symbol={symbol}&limit=50"
            signature = base64.b64encode(
                hmac.new(api_secret.encode(), (timestamp + "GET" + path).encode(), hashlib.sha256).digest()
            ).decode()
            req = urllib.request.Request(
                f"https://api.bitget.com{path}",
                headers={"ACCESS-KEY": api_key, "ACCESS-SIGN": signature,
                         "ACCESS-TIMESTAMP": timestamp, "ACCESS-PASSPHRASE": passphrase,
                         "Content-Type": "application/json"}
            )
            with urllib.request.urlopen(req, timeout=5) as r:
                data = json.loads(r.read())
            for o in (data.get("data") or []):
                try:
                    ts_order = int(o.get("cTime", 0))
                    if ts_order < START_TS: continue
                    if o.get("status") not in ("filled", "full_fill"): continue
                    qty = float(o.get("baseVolume", 0))
                    total = float(o.get("quoteVolume", 0))
                    orders.append({
                        "exchange": "bitget",
                        "symbol": symbol.replace("USDT", ""),
                        "side": o.get("side", "").upper(),
                        "qty": qty,
                        "price": total / qty if qty > 0 else 0,
                        "total": total,
                        "time": ts_order,
                        "status": "FILLED",
                    })
                except: pass
    except Exception:
        pass

    # Coinbase
    try:
        from coinbase.rest import RESTClient
        api_key = env.get("COINBASE_API_KEY", "")
        api_secret_path = env.get("COINBASE_API_SECRET", "")
        secret = (Path(api_secret_path).read_text().strip()
                  if Path(api_secret_path).exists() else api_secret_path)
        client = RESTClient(api_key=api_key, api_secret=secret)
        result = client.list_orders(order_status=["FILLED"], limit=50)
        for o in result.orders:
            try:
                ts_order = int(time.mktime(time.strptime(o.created_time[:19], "%Y-%m-%dT%H:%M:%S"))) * 1000
                if ts_order < START_TS: continue
                qty = float(o.filled_size or 0)
                total = float(o.filled_value or 0)
                orders.append({
                    "exchange": "coinbase",
                    "symbol": o.product_id.split("-")[0],
                    "side": o.side,
                    "qty": qty,
                    "price": total / qty if qty > 0 else 0,
                    "total": total,
                    "time": ts_order,
                    "status": "FILLED",
                })
            except: pass
    except Exception:
        pass

    orders.sort(key=lambda x: x["time"], reverse=True)
    return orders

# ---------------------------------------------------------------------------
# Balances
# ---------------------------------------------------------------------------

def get_balances():
    global _balance_cache
    now = time.time()
    if now - _balance_cache["ts"] < 60 and _balance_cache["data"]:
        return _balance_cache["data"]
    env = read_env()
    result = {
        "coinbase": get_coinbase_balance(env.get("COINBASE_API_KEY", ""), env.get("COINBASE_API_SECRET", "")),
        "binance":  get_binance_balance(env.get("BINANCE_API_KEY", ""), env.get("BINANCE_API_SECRET", "")),
        "bitget":   get_bitget_balance(env.get("BITGET_API_KEY", ""), env.get("BITGET_API_SECRET", ""), env.get("BITGET_PASSPHRASE", "")),
        "bybit":    get_bybit_balance(env.get("BYBIT_API_KEY", ""), env.get("BYBIT_API_SECRET", "")),
        "updated_at": int(now),
    }
    _balance_cache = {"ts": now, "data": result}
    return result

# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def calc_trading_pnl():
    try:
        conn = sqlite3.connect(DB)
        tables = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()]
        if 'trades' not in tables:
            conn.close()
            return 0.0
        result = conn.execute("SELECT SUM(profit_usd) FROM trades WHERE status='SUCCESS'").fetchone()
        conn.close()
        return round(result[0] or 0.0, 4)
    except Exception:
        return 0.0

def get_data():
    if not Path(DB).exists():
        return None
    env = read_env()
    capital = float(env.get("CAPITAL_PER_EXCHANGE", 500))
    conn = sqlite3.connect(DB)
    conn.row_factory = sqlite3.Row
    opps = [dict(r) for r in conn.execute("""
        SELECT * FROM opportunities WHERE profit_pct >= 0.01
        AND buy_exchange IN ('coinbase', 'binance', 'bitget')
        AND sell_exchange IN ('coinbase', 'binance', 'bitget')
        AND signal_score >= 4
        ORDER BY timestamp_ms DESC LIMIT 1000
    """).fetchall()]
    spreads = [dict(r) for r in conn.execute("""
        SELECT * FROM spreads ORDER BY timestamp_ms DESC LIMIT 100
    """).fetchall()]
    stats = dict(conn.execute(f"""
        SELECT COUNT(*) as total,
            AVG(profit_pct) as avg_profit, MAX(profit_pct) as max_profit,
            AVG(signal_duration_ms) as avg_dur,
            AVG(normalized_spread_pct) as avg_norm, AVG(raw_spread_pct) as avg_raw,
            SUM(MIN({capital}, max_size_usd) * profit_pct / 100.0) as real_profit_usd
        FROM (
            SELECT buy_exchange, sell_exchange, symbol,
                MAX(profit_pct) as profit_pct,
                MAX(normalized_spread_pct) as normalized_spread_pct,
                MAX(raw_spread_pct) as raw_spread_pct,
                MAX(signal_duration_ms) as signal_duration_ms,
                MAX(max_size_usd) as max_size_usd,
                signal_status
            FROM opportunities
            WHERE profit_pct >= 0.01 AND signal_status != 'weak'
            AND buy_exchange IN ('coinbase', 'binance', 'bitget')
            AND sell_exchange IN ('coinbase', 'binance', 'bitget')
            AND signal_score >= 4
            GROUP BY buy_exchange, sell_exchange, symbol, CAST(timestamp_ms / 30000 AS INTEGER)
        )
    """).fetchone())
    by_sym = [dict(r) for r in conn.execute("""
        SELECT symbol, COUNT(*) as cnt, AVG(profit_pct) as avg_profit,
            MAX(profit_pct) as max_profit, AVG(signal_duration_ms)/1000.0 as avg_dur_s
        FROM opportunities WHERE profit_pct >= 0.01 AND signal_status != 'weak'
        AND buy_exchange IN ('coinbase', 'binance', 'bitget')
        AND sell_exchange IN ('coinbase', 'binance', 'bitget')
        AND signal_score >= 4
        GROUP BY symbol ORDER BY cnt DESC
    """).fetchall()]
    by_pair = [dict(r) for r in conn.execute("""
        SELECT buy_exchange || '→' || sell_exchange as pair, symbol,
            COUNT(*) as cnt, AVG(profit_pct) as avg_profit, MAX(profit_pct) as max_profit
        FROM opportunities WHERE profit_pct >= 0.01 AND signal_status != 'weak'
        AND buy_exchange IN ('coinbase', 'binance', 'bitget')
        AND sell_exchange IN ('coinbase', 'binance', 'bitget')
        AND signal_score >= 4
        GROUP BY pair, symbol ORDER BY cnt DESC LIMIT 10
    """).fetchall()]
    dur_data = conn.execute("""
        SELECT signal_duration_ms FROM opportunities
        WHERE profit_pct >= 0.01 AND signal_status != 'weak'
        AND buy_exchange IN ('coinbase', 'binance', 'bitget')
        AND sell_exchange IN ('coinbase', 'binance', 'bitget')
        AND signal_score >= 4
    """).fetchall()
    buckets = {'<1s': 0, '1-5s': 0, '5-15s': 0, '>15s': 0}
    for (d,) in dur_data:
        if d < 1000: buckets['<1s'] += 1
        elif d < 5000: buckets['1-5s'] += 1
        elif d < 15000: buckets['5-15s'] += 1
        else: buckets['>15s'] += 1
    conn.close()
    return {
        'opps': opps[:50], 'spreads': spreads, 'stats': stats,
        'capital': capital, 'by_sym': by_sym, 'by_pair': by_pair,
        'dur_buckets': buckets, 'actual_profit_usd': calc_trading_pnl(),
    }

def get_trades_data():
    if not Path(DB).exists():
        return {'trades': [], 'summary': {}, 'liq_summary': {}, 'liq_sales': []}
    try:
        conn = sqlite3.connect(DB)
        conn.row_factory = sqlite3.Row
        tables = [r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()]
        if 'trades' not in tables:
            conn.close()
            return {'trades': [], 'summary': {}, 'liq_summary': {}, 'liq_sales': []}

        trades = [dict(r) for r in conn.execute("SELECT * FROM trades ORDER BY timestamp_ms DESC LIMIT 200").fetchall()]
        summary = dict(conn.execute("""
            SELECT COUNT(*) as total,
                SUM(CASE WHEN status='SUCCESS'    THEN 1 ELSE 0 END) as success,
                SUM(CASE WHEN status='BUY_FAILED' THEN 1 ELSE 0 END) as buy_failed,
                SUM(CASE WHEN status NOT IN ('SUCCESS','BUY_FAILED') THEN 1 ELSE 0 END) as other_failed,
                SUM(CASE WHEN status='SUCCESS' THEN profit_usd     ELSE 0 END) as total_profit,
                SUM(CASE WHEN status='SUCCESS' THEN trade_size_usd ELSE 0 END) as total_volume,
                AVG(CASE WHEN status='SUCCESS' THEN profit_usd END) as avg_profit,
                AVG(CASE WHEN status='SUCCESS' THEN latency_ms END) as avg_latency
            FROM trades
        """).fetchone())
        by_pair = [dict(r) for r in conn.execute("""
            SELECT buy_exchange || ' -> ' || sell_exchange as pair,
                COUNT(*) as total,
                SUM(CASE WHEN status='SUCCESS' THEN 1          ELSE 0 END) as success,
                SUM(CASE WHEN status='SUCCESS' THEN profit_usd ELSE 0 END) as profit,
                SUM(CASE WHEN status='SUCCESS' THEN trade_size_usd ELSE 0 END) as volume
            FROM trades GROUP BY pair ORDER BY profit DESC
        """).fetchall()]

        # Liquidity sales
        liq_summary = {}
        liq_sales = []
        liq_by_exchange = []

        if "liquidity_sales" in tables:
            liq_summary = dict(conn.execute("""
                SELECT
                    COUNT(*)                                                              AS total_sales,
                    SUM(CASE WHEN status='SUCCESS' THEN 1 ELSE 0 END)                    AS success_sales,
                    SUM(CASE WHEN status='FAILED'  THEN 1 ELSE 0 END)                    AS failed_sales,
                    SUM(CASE WHEN status='SUCCESS' THEN pnl_usd ELSE 0 END)              AS total_pnl,
                    AVG(CASE WHEN status='SUCCESS' THEN pnl_usd END)                     AS avg_pnl,
                    SUM(CASE WHEN status='SUCCESS' AND pnl_usd > 0  THEN 1 ELSE 0 END)  AS pnl_positive,
                    SUM(CASE WHEN status='SUCCESS' AND pnl_usd = 0  THEN 1 ELSE 0 END)  AS pnl_zero,
                    SUM(CASE WHEN status='SUCCESS' AND pnl_usd < 0  THEN 1 ELSE 0 END)  AS pnl_negative,
                    SUM(CASE WHEN sale_type='LIQUIDITY_RECOVERY' AND status='SUCCESS' THEN 1 ELSE 0 END) AS liq_recovery_cnt,
                    SUM(CASE WHEN sale_type='STUCK_EXIT'         AND status='SUCCESS' THEN 1 ELSE 0 END) AS stuck_exit_cnt
                FROM liquidity_sales
            """).fetchone())
            liq_sales = [dict(r) for r in conn.execute("""
                SELECT timestamp_ms, sale_type, exchange, symbol, qty,
                    buy_price, sell_price, buy_cost_usd, sell_recv_usd,
                    pnl_usd, pnl_pct, status
                FROM liquidity_sales ORDER BY timestamp_ms DESC LIMIT 100
            """).fetchall()]
            liq_by_exchange = [dict(r) for r in conn.execute("""
                SELECT exchange, symbol, sale_type,
                    COUNT(*)                                                            AS cnt,
                    SUM(CASE WHEN status='SUCCESS' THEN pnl_usd ELSE 0 END)            AS total_pnl,
                    AVG(CASE WHEN status='SUCCESS' THEN pnl_usd END)                   AS avg_pnl,
                    SUM(CASE WHEN status='SUCCESS' AND pnl_usd > 0  THEN 1 ELSE 0 END) AS profit_cnt,
                    SUM(CASE WHEN status='SUCCESS' AND pnl_usd <= 0 THEN 1 ELSE 0 END) AS zero_or_loss_cnt
                FROM liquidity_sales WHERE status='SUCCESS'
                GROUP BY exchange, symbol, sale_type ORDER BY exchange, symbol
            """).fetchall()]

        conn.close()
        return {
            'trades': trades, 'summary': summary, 'by_pair': by_pair,
            'liq_summary': liq_summary, 'liq_sales': liq_sales,
            'liq_by_exchange': liq_by_exchange,
        }
    except Exception as e:
        return {'trades': [], 'summary': {}, 'liq_summary': {}, 'liq_sales': [], 'error': str(e)}

# ---------------------------------------------------------------------------
# Logs
# ---------------------------------------------------------------------------

_ANSI_RE = re.compile(r'\x1b\[[0-9;]*m')
_TS_RE = re.compile(r'^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})\.\d+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)')
_EXEC_KEYS = ('[EXEC]', '[exec]', 'coinbase-exec', 'binance-exec', 'bitget-exec', '[INV-OPT]', '[STUCK]')

def _tail_file(path, n):
    """Memory-efficient tail: read last n lines."""
    with open(path, 'rb') as f:
        chunk = 65536
        lines_found = []
        remainder = b''
        f.seek(0, 2)
        pos = f.tell()
        while pos > 0 and len(lines_found) < n:
            read_size = min(chunk, pos)
            pos -= read_size
            f.seek(pos)
            data = f.read(read_size) + remainder
            lines_in_chunk = data.split(b'\n')
            remainder = lines_in_chunk[0]
            lines_found = lines_in_chunk[1:] + lines_found
        if remainder:
            lines_found = [remainder] + lines_found
    tail = lines_found[-n:] if len(lines_found) > n else lines_found
    return [l.decode('utf-8', errors='replace') for l in tail]

def get_logs_data(lines=500, level_filter=None, q=None, page=1, size=200):
    log_path = ROOT_DIR / 'bot.log'
    if not log_path.exists():
        return {'lines': [], 'total': 0, 'pages': 0, 'page': 1, 'error': 'bot.log not found'}
    try:
        raw = _tail_file(log_path, lines)
        result = []
        for raw_line in raw:
            raw_line = _ANSI_RE.sub('', raw_line).rstrip()
            if not raw_line: continue
            m = _TS_RE.match(raw_line)
            if m:
                ts_utc, level, msg = m.group(1), m.group(2), m.group(3)
                if level_filter:
                    if level_filter == 'EXEC':
                        if not any(k in msg for k in _EXEC_KEYS): continue
                    elif level != level_filter:
                        continue
                if q and q.lower() not in msg.lower() and q.lower() not in level.lower():
                    continue
                result.append({'ts': ts_utc, 'level': level, 'msg': msg})
            else:
                if result:
                    result[-1]['msg'] += '\n' + raw_line
                else:
                    if not level_filter and (not q or q.lower() in raw_line.lower()):
                        result.append({'ts': '', 'level': 'INFO', 'msg': raw_line})

        total = len(result)
        pages = max(1, (total + size - 1) // size)
        page = min(page, pages)
        start = (page - 1) * size
        return {
            'lines': result[start:start + size],
            'total': total, 'page': page, 'pages': pages, 'size': size,
        }
    except Exception as e:
        return {'lines': [], 'total': 0, 'pages': 0, 'page': 1, 'error': str(e)}

# ---------------------------------------------------------------------------
# Start balance
# ---------------------------------------------------------------------------

START_BALANCE_FILE = ROOT_DIR / 'start_balance.json'

def get_or_set_start_balance(current_balances):
    if START_BALANCE_FILE.exists():
        try:
            return json.loads(START_BALANCE_FILE.read_text())
        except: pass
    keys = ['coinbase', 'binance', 'bitget', 'bybit']
    if current_balances and all(current_balances.get(k) is not None for k in keys):
        total = sum(current_balances.get(k, 0) or 0 for k in keys)
        data = {**current_balances, 'total': total, 'saved_at': int(time.time())}
        START_BALANCE_FILE.write_text(json.dumps(data))
        return data
    return None

# ---------------------------------------------------------------------------
# HTML templates (inline)
# ---------------------------------------------------------------------------

_CSS = '''
:root{--bg:#0f1117;--bg2:#1a1d27;--bg3:#242736;--border:#2e3347;--text:#e8eaf0;--muted:#7b8099;--green:#22c55e;--blue:#3b82f6;--amber:#f59e0b;--red:#ef4444;--cyan:#06b6d4}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--text);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:14px;line-height:1.5}
.header{padding:16px 24px;border-bottom:1px solid var(--border);display:flex;justify-content:space-between;align-items:center}
.header h1{font-size:18px;font-weight:600}
.nav a{color:var(--muted);text-decoration:none;padding:6px 14px;border-radius:6px;font-size:13px;border:1px solid var(--border);margin-left:6px}
.nav a:hover,.nav a.active{color:var(--text);border-color:var(--blue)}
.container{padding:20px 24px}
.grid4{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:16px}
.grid3{display:grid;grid-template-columns:repeat(3,1fr);gap:12px;margin-bottom:16px}
.grid2{display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-bottom:16px}
.metric{background:var(--bg2);border:1px solid var(--border);border-radius:10px;padding:14px 16px}
.metric-label{font-size:11px;color:var(--muted);text-transform:uppercase;letter-spacing:.06em;margin-bottom:6px}
.metric-value{font-size:24px;font-weight:600}
.metric-sub{font-size:11px;color:var(--muted);margin-top:3px}
.card{background:var(--bg2);border:1px solid var(--border);border-radius:10px;padding:16px;margin-bottom:16px}
.card-title{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);margin-bottom:14px}
.chart-wrap{position:relative;height:180px}
table{width:100%;border-collapse:collapse;font-size:13px}
th{text-align:left;color:var(--muted);font-size:11px;text-transform:uppercase;letter-spacing:.05em;padding:6px 8px;border-bottom:1px solid var(--border);font-weight:500}
td{padding:8px;border-bottom:1px solid var(--border)}
tr:last-child td{border-bottom:none}
tr:hover td{background:var(--bg3)}
.green{color:var(--green)}.blue{color:var(--blue)}.amber{color:var(--amber)}.red{color:var(--red)}
.badge{padding:2px 7px;border-radius:4px;font-size:11px;font-weight:500}
.badge-strong{background:rgba(34,197,94,.15);color:#22c55e}
.badge-medium{background:rgba(245,158,11,.15);color:#f59e0b}
.badge-weak{background:rgba(239,68,68,.15);color:#ef4444}
.live{display:flex;align-items:center;gap:6px;font-size:12px;color:var(--green)}
.dot{width:8px;height:8px;border-radius:50%;background:var(--green);animation:pulse 2s infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.4}}
.usdt-badge{font-size:12px;padding:3px 8px;border-radius:4px;background:rgba(59,130,246,.15);color:var(--blue)}
.btn{background:var(--bg3);border:1px solid var(--border);color:var(--text);padding:6px 14px;border-radius:6px;cursor:pointer;font-size:13px}
.btn:hover{border-color:var(--blue);color:var(--blue)}
.balance-row{display:flex;align-items:center;justify-content:space-between;padding:10px 0;border-bottom:1px solid var(--border)}
.balance-row:last-child{border-bottom:none}
.exchange-name{font-weight:500;font-size:14px}
.balance-amount{font-size:18px;font-weight:600;color:var(--green)}
.balance-currency{font-size:11px;color:var(--muted);margin-left:4px}
.profit-section{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin-top:12px}
.profit-box{background:var(--bg3);border-radius:8px;padding:12px}
.profit-box-label{font-size:11px;color:var(--muted);margin-bottom:4px}
.profit-box-value{font-size:20px;font-weight:600}
.bar-row{display:flex;align-items:center;gap:10px;margin-bottom:10px;font-size:13px}
.bar-label{width:150px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.bar-track{flex:1;height:6px;background:var(--bg3);border-radius:3px;overflow:hidden}
.bar-fill{height:100%;border-radius:3px;background:var(--blue)}
.bar-val{width:70px;text-align:right;font-size:12px;color:var(--green)}
.loading{color:var(--muted);font-size:13px}
.error{color:var(--red);font-size:12px}
.day-header{background:var(--bg3);padding:10px 16px;border-radius:8px;margin:16px 0 8px;display:flex;justify-content:space-between;align-items:center}
.day-title{font-weight:600;font-size:14px}
.day-stats{display:flex;gap:16px;font-size:12px;color:var(--muted)}
.b{padding:2px 7px;border-radius:4px;font-size:11px;font-weight:500}
.bok{background:rgba(34,197,94,.15);color:#22c55e}
.bfl{background:rgba(239,68,68,.15);color:#ef4444}
.bsk{background:rgba(245,158,11,.15);color:#f59e0b}
.bliq{background:rgba(6,182,212,.15);color:#06b6d4}
.bstuck{background:rgba(245,158,11,.15);color:#f59e0b}
.toolbar{display:flex;gap:8px;margin-bottom:16px;align-items:center;flex-wrap:wrap}
.fbtn{padding:5px 12px;border-radius:6px;font-size:12px;font-weight:500;border:1px solid var(--border);background:var(--bg2);color:var(--muted);cursor:pointer}
.fbtn:hover,.fbtn.on{color:var(--text);border-color:var(--blue);background:var(--bg3)}
.fbtn.on{background:rgba(59,130,246,.15)}
.search{padding:5px 12px;border-radius:6px;font-size:12px;border:1px solid var(--border);background:var(--bg2);color:var(--text);width:220px;outline:none}
.search:focus{border-color:var(--blue)}
.log-wrap{background:var(--bg2);border:1px solid var(--border);border-radius:10px;padding:12px;max-height:calc(100vh - 230px);overflow-y:auto;font-family:"JetBrains Mono","Fira Code",monospace;font-size:12px;line-height:1.7}
.log-line{display:flex;gap:10px;padding:1px 4px;border-radius:3px}
.log-line:hover{background:var(--bg3)}
.log-ts{color:var(--muted);white-space:nowrap;min-width:70px}
.log-lvl{font-weight:600;min-width:44px;text-align:center}
.log-msg{word-break:break-all;white-space:pre-wrap}
.lvl-ERROR{color:var(--red)}.lvl-WARN{color:var(--amber)}.lvl-INFO{color:var(--cyan)}.lvl-DEBUG{color:var(--muted)}.lvl-TRACE{color:var(--muted)}
.err-line{background:rgba(239,68,68,.06)}.warn-line{background:rgba(245,158,11,.04)}
.pagination{display:flex;align-items:center;gap:8px;margin-top:12px;font-size:13px;color:var(--muted)}
.pagination button{padding:4px 12px;border-radius:6px;font-size:12px;border:1px solid var(--border);background:var(--bg2);color:var(--text);cursor:pointer}
.pagination button:hover:not(:disabled){border-color:var(--blue)}
.pagination button:disabled{opacity:.35;cursor:default}
.pg-info{font-size:12px}
.section-divider{display:flex;align-items:center;gap:12px;margin:24px 0 14px}
.section-divider::before,.section-divider::after{content:'';flex:1;height:1px;background:var(--border)}
.section-label{font-size:11px;font-weight:600;text-transform:uppercase;letter-spacing:.07em;color:var(--muted);white-space:nowrap;padding:0 8px}
.liq-breakdown{margin-top:6px}
.liq-row{font-size:13px;line-height:1.8}
'''

_NAV = '''<nav class="nav">
  <a href="/" {d}>Dashboard</a>
  <a href="/trades" {t}>Trades</a>
  <a href="/logs" {l}>Logs</a>
</nav>'''

def _nav(active):
    return _NAV.format(
        d='class="active"' if active == 'd' else '',
        t='class="active"' if active == 't' else '',
        l='class="active"' if active == 'l' else '',
    )

HTML = '''<!DOCTYPE html>
<html lang="ru"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Crypto Arb Dashboard</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
<style>''' + _CSS + '''</style></head><body>
<div class="header">
  <h1>Crypto Arb Scanner</h1>
  <div style="display:flex;gap:12px;align-items:center">
    <span id="usdt-badge" class="usdt-badge">USDT —</span>
    <div class="live"><div class="dot"></div><span id="last-update">—</span></div>
    <button class="btn" onclick="loadData()">Обновить</button>
    ''' + _nav('d') + '''
  </div>
</div>
<div class="container">
<div class="grid4">
  <div class="metric"><div class="metric-label">Всего сигналов</div><div class="metric-value green" id="m-total">—</div><div class="metric-sub">уникальных окон</div></div>
  <div class="metric"><div class="metric-label">Avg profit (norm)</div><div class="metric-value blue" id="m-avg-profit">—</div><div class="metric-sub" id="m-raw-spread">raw —</div></div>
  <div class="metric"><div class="metric-label">Max profit</div><div class="metric-value amber" id="m-max-profit">—</div><div class="metric-sub">лучший сигнал</div></div>
  <div class="metric"><div class="metric-label">Реальная прибыль</div><div class="metric-value green" id="profit-real">—</div><div class="metric-sub" id="profit-real-sub">расчёт системы</div></div>
</div>
<div class="grid2">
  <div class="card">
    <div class="card-title">Балансы бирж (обновление раз в минуту)</div>
    <div id="balances-content"><div class="loading">Загрузка...</div></div>
    <div class="profit-section" style="grid-template-columns:repeat(3,1fr)">
      <div class="profit-box"><div class="profit-box-label">Приблизительная</div><div class="profit-box-value green" id="profit-est">$0.00</div><div style="font-size:11px;color:var(--muted);margin-top:4px">по сигналам бота</div></div>
      <div class="profit-box"><div class="profit-box-label">Холд</div><div class="profit-box-value green" id="profit-hold">$—</div><div style="font-size:11px;color:var(--muted);margin-top:4px">рост цены крипты</div></div>
      <div class="profit-box"><div class="profit-box-label">Трейдинг</div><div class="profit-box-value green" id="profit-trading">$—</div><div style="font-size:11px;color:var(--muted);margin-top:4px">арбитраж сделки</div></div>
    </div>
  </div>
  <div class="card">
    <div class="card-title">Raw vs Normalized spread</div>
    <div class="chart-wrap"><canvas id="spreadChart"></canvas></div>
    <div id="spread-stats" style="display:grid;grid-template-columns:repeat(4,1fr);gap:8px;margin-top:12px;padding-top:10px;border-top:1px solid var(--border)">
      <div style="text-align:center"><div style="font-size:10px;color:var(--muted);margin-bottom:3px">ТЕКУЩИЙ RAW</div><div id="ss-current" style="font-size:14px;font-weight:600">—</div></div>
      <div style="text-align:center"><div style="font-size:10px;color:var(--muted);margin-bottom:3px">МАКС RAW</div><div id="ss-max" style="font-size:14px;font-weight:600;color:var(--amber)">—</div></div>
      <div style="text-align:center"><div style="font-size:10px;color:var(--muted);margin-bottom:3px">USDT ПРЕМИЯ</div><div id="ss-premium" style="font-size:14px;font-weight:600;color:var(--blue)">—</div></div>
      <div style="text-align:center"><div style="font-size:10px;color:var(--muted);margin-bottom:3px">ДО ПОРОГА</div><div id="ss-gap" style="font-size:14px;font-weight:600">—</div></div>
    </div>
  </div>
</div>
<div class="grid3">
  <div class="card"><div class="card-title">По символам</div><div class="chart-wrap"><canvas id="symChart"></canvas></div></div>
  <div class="card"><div class="card-title">Profit distribution</div><div class="chart-wrap"><canvas id="profitChart"></canvas></div></div>
  <div class="card"><div class="card-title">Signal duration</div><div class="chart-wrap"><canvas id="durChart"></canvas></div></div>
</div>
<div class="card"><div class="card-title">Топ пары</div><div id="pairs-bars"></div></div>
<div class="card">
  <div class="card-title" style="display:flex;justify-content:space-between;align-items:center">
    <span>Реальные ордера (с 1 апреля)</span><span id="orders-count" style="font-size:11px;color:var(--muted)">загрузка...</span>
  </div>
  <table><thead><tr><th>Время</th><th>Биржа</th><th>Символ</th><th>Сторона</th><th>Количество</th><th>Цена</th><th>Итого</th></tr></thead>
  <tbody id="orders-table"><tr><td colspan="7" style="color:var(--muted);text-align:center">Загрузка...</td></tr></tbody></table>
</div>
<div class="card">
  <div class="card-title">Последние сигналы</div>
  <table><thead><tr><th>Время</th><th>Пара</th><th>Символ</th><th>Profit</th><th>Norm spread</th><th>Size</th><th>Duration</th><th>Score</th><th>Status</th></tr></thead>
  <tbody id="signals-table"></tbody></table>
</div>
</div>
<script>
const charts={};
function fmt(n,dec=4){return(+n).toFixed(dec);}
function fmtMs(ms){return ms<1000?ms+'ms':(ms/1000).toFixed(1)+'s';}
function fmtTime(ts){return new Date(ts).toLocaleString('ru-RU',{timeZone:'Asia/Jerusalem',day:'2-digit',month:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit',hour12:false});}
function destroyChart(id){if(charts[id]){charts[id].destroy();delete charts[id];}}

function loadBalances(){
  fetch('/balances').then(r=>r.json()).then(renderBalances)
  .catch(()=>{document.getElementById('balances-content').innerHTML='<div class="error">Ошибка загрузки балансов</div>';});
}
function renderBalances(b){
  const exchanges=[{key:'coinbase',name:'Coinbase',currency:'USD/USDC'},{key:'binance',name:'Binance',currency:'USDT'},{key:'bitget',name:'Bitget',currency:'USDT'},{key:'bybit',name:'Bybit',currency:'USDT'}];
  let html='',total=0,allLoaded=true;
  exchanges.forEach(ex=>{
    const val=b[ex.key];
    if(val===null||val===undefined){allLoaded=false;html+=`<div class="balance-row"><span class="exchange-name">${ex.name}</span><span class="error">нет данных</span></div>`;}
    else{total+=val;html+=`<div class="balance-row"><span class="exchange-name">${ex.name}</span><span><span class="balance-amount">$${val.toFixed(2)}</span><span class="balance-currency">${ex.currency}</span></span></div>`;}
  });
  if(allLoaded){
    html+=`<div class="balance-row" style="border-top:1px solid var(--border);margin-top:4px;padding-top:10px"><span class="exchange-name" style="color:var(--muted)">ИТОГО</span><span class="balance-amount">$${total.toFixed(2)}</span></div>`;
    fetch('/start_balance').then(r=>r.json()).then(sb=>{
      if(sb&&sb.total){
        const totalProfit=total-sb.total;
        fetch('/data').then(r=>r.json()).then(d=>{
          const tradingProfit=(d&&d.actual_profit_usd)||0;
          const holdProfit=totalProfit-tradingProfit;
          const fmtP=v=>(v>=0?'+':'-')+'$'+Math.abs(v).toFixed(2);
          const cls=v=>'profit-box-value '+(v>=0?'green':'red');
          const holdEl=document.getElementById('profit-hold');holdEl.textContent=fmtP(holdProfit);holdEl.className=cls(holdProfit);
          const tradingEl=document.getElementById('profit-trading');tradingEl.textContent=fmtP(tradingProfit);tradingEl.className=cls(tradingProfit);
          const totalEl=document.getElementById('profit-real');totalEl.textContent=fmtP(totalProfit);totalEl.className=cls(totalProfit);
        });
        document.getElementById('profit-real-sub').textContent=`старт: $${sb.total.toFixed(2)} → сейчас: $${total.toFixed(2)}`;
      }else{document.getElementById('profit-real-sub').textContent='стартовый баланс не задан';}
    });
  }
  const upd=new Date(b.updated_at*1000).toLocaleTimeString('ru-RU');
  document.getElementById('balances-content').innerHTML=html+`<div style="font-size:11px;color:var(--muted);margin-top:8px">обновлено: ${upd}</div>`;
}
function loadData(){fetch('/data').then(r=>r.json()).then(d=>{if(d)renderData(d);}).catch(e=>console.error(e));}
function renderData(d){
  const s=d.stats;
  document.getElementById('m-total').textContent=s.total||0;
  document.getElementById('m-avg-profit').textContent=fmt(s.avg_profit||0)+'%';
  document.getElementById('m-raw-spread').textContent='raw '+fmt(s.avg_raw||0)+'%';
  document.getElementById('m-max-profit').textContent=fmt(s.max_profit||0)+'%';
  document.getElementById('profit-est').textContent='$'+fmt(s.real_profit_usd||0,2);
  document.getElementById('last-update').textContent=new Date().toLocaleTimeString('ru-RU');
  if(d.opps.length){const rate=d.opps[0].usdt_rate;const dev=((rate-1)*100).toFixed(4);document.getElementById('usdt-badge').textContent=`USDT ${rate} (${dev>0?'+':''}${dev}%)`;}
  destroyChart('sym');charts['sym']=new Chart(document.getElementById('symChart'),{type:'bar',data:{labels:d.by_sym.map(r=>r.symbol),datasets:[{data:d.by_sym.map(r=>r.cnt),backgroundColor:['#22c55e','#3b82f6','#f59e0b','#a855f7','#ec4899'],borderRadius:4,borderSkipped:false}]},options:{responsive:true,maintainAspectRatio:false,plugins:{legend:{display:false}},scales:{x:{grid:{display:false},ticks:{color:'#7b8099'}},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099',stepSize:1}}}}});
  const pb={'0.01-0.02':0,'0.02-0.04':0,'0.04-0.07':0,'>0.07':0};
  d.opps.forEach(o=>{const p=o.profit_pct;if(p<0.02)pb['0.01-0.02']++;else if(p<0.04)pb['0.02-0.04']++;else if(p<0.07)pb['0.04-0.07']++;else pb['>0.07']++;});
  destroyChart('profit');charts['profit']=new Chart(document.getElementById('profitChart'),{type:'bar',data:{labels:Object.keys(pb),datasets:[{data:Object.values(pb),backgroundColor:'#3b82f6',borderRadius:4,borderSkipped:false}]},options:{responsive:true,maintainAspectRatio:false,plugins:{legend:{display:false}},scales:{x:{grid:{display:false},ticks:{color:'#7b8099'}},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099',stepSize:1}}}}});
  destroyChart('dur');charts['dur']=new Chart(document.getElementById('durChart'),{type:'doughnut',data:{labels:Object.keys(d.dur_buckets),datasets:[{data:Object.values(d.dur_buckets),backgroundColor:['#ef4444','#f59e0b','#22c55e','#3b82f6'],borderWidth:0}]},options:{responsive:true,maintainAspectRatio:false,plugins:{legend:{position:'bottom',labels:{color:'#7b8099',font:{size:11},boxWidth:10,padding:8}}}}});
  const sp=d.spreads.length>0?d.spreads.slice(0,30).reverse():d.opps.slice(0,20).reverse();
  const THRESHOLD=0.15;
  const rawVals=sp.map(r=>+fmt(r.raw_spread_pct));const normVals=sp.map(r=>+fmt(r.normalized_spread_pct||r.raw_spread_pct*0.78));
  const pointColors=rawVals.map(v=>v>=THRESHOLD?'#ffffff':'rgba(255,255,255,0.2)');const pointSizes=rawVals.map(v=>v>=THRESHOLD?6:2);
  destroyChart('spread');charts['spread']=new Chart(document.getElementById('spreadChart'),{type:'line',data:{labels:sp.map((_,i)=>i+1),datasets:[{label:'Raw %',data:rawVals,borderColor:'#ef4444',backgroundColor:'rgba(239,68,68,0.08)',tension:0.3,fill:true,pointRadius:pointSizes,pointBackgroundColor:pointColors,pointBorderColor:pointColors},{label:'Norm %',data:normVals,borderColor:'#22c55e',backgroundColor:'rgba(34,197,94,0.08)',tension:0.3,fill:true,pointRadius:2,pointBackgroundColor:'#22c55e'},{label:'Порог 0.15%',data:sp.map(()=>THRESHOLD),borderColor:'rgba(255,255,255,0.35)',borderDash:[4,4],borderWidth:1,pointRadius:0,fill:false,tension:0}]},plugins:[{id:'spreadLabels',afterDatasetsDraw(chart){const ctx2=chart.ctx;const meta=chart.getDatasetMeta(0);ctx2.save();meta.data.forEach((pt,i)=>{const v=rawVals[i];const isPeak=(i===0||v>=rawVals[i-1])&&(i===rawVals.length-1||v>=rawVals[i+1])&&v>0.015;const isThreshold=v>=THRESHOLD;if(!isPeak&&!isThreshold)return;ctx2.font=isThreshold?'bold 10px sans-serif':'9px sans-serif';ctx2.fillStyle=isThreshold?'#ffffff':'rgba(255,255,255,0.4)';ctx2.textAlign='center';ctx2.fillText(v.toFixed(3)+'%',pt.x,pt.y-8);});ctx2.restore();}}],options:{responsive:true,maintainAspectRatio:false,plugins:{legend:{labels:{color:'#7b8099',font:{size:11},boxWidth:10}},tooltip:{callbacks:{label:ctx=>{const v=ctx.parsed.y;const status=v>=THRESHOLD?' ✅ ТОРГУЕМ':' ⬜ ниже порога';return` ${ctx.dataset.label}: ${v.toFixed(4)}%${ctx.datasetIndex===0?status:''}`;}}}},scales:{x:{display:false},y:{grid:{color:'rgba(255,255,255,0.05)'},ticks:{color:'#7b8099',callback:v=>v.toFixed(3)+'%'}}}}});
  if(sp.length>0){
    const lastRaw=rawVals[rawVals.length-1];const normLast=normVals[normVals.length-1];const maxRaw=Math.max(...rawVals);const premium=lastRaw-normLast;const gap=THRESHOLD-lastRaw;
    const curEl=document.getElementById('ss-current');curEl.textContent=lastRaw.toFixed(4)+'%';curEl.style.color=lastRaw>=THRESHOLD?'var(--green)':'var(--text)';
    document.getElementById('ss-max').textContent=maxRaw.toFixed(4)+'%';document.getElementById('ss-premium').textContent=premium.toFixed(4)+'%';
    const gapEl=document.getElementById('ss-gap');if(gap<=0){gapEl.textContent='✅ выше порога';gapEl.style.color='var(--green)';}else{gapEl.textContent='+'+gap.toFixed(4)+'%';gapEl.style.color=gap<0.05?'var(--amber)':'var(--muted)';}
  }
  const pairEl=document.getElementById('pairs-bars');pairEl.innerHTML='';const maxCnt=Math.max(...d.by_pair.map(r=>r.cnt),1);
  d.by_pair.slice(0,8).forEach(r=>{pairEl.innerHTML+=`<div class="bar-row"><span class="bar-label">${r.pair} ${r.symbol}</span><div class="bar-track"><div class="bar-fill" style="width:${r.cnt/maxCnt*100}%"></div></div><span class="bar-val">+${fmt(r.max_profit)}%</span></div>`;});
  const tbody=document.getElementById('signals-table');tbody.innerHTML='';
  d.opps.slice(0,20).forEach(o=>{const sc=o.signal_status==='strong'?'strong':o.signal_status==='medium'?'medium':'weak';tbody.innerHTML+=`<tr><td style="color:#7b8099">${fmtTime(o.timestamp_ms)}</td><td>${o.buy_exchange} → ${o.sell_exchange}</td><td>${o.symbol}</td><td style="color:#22c55e;font-weight:500">+${fmt(o.profit_pct)}%</td><td>${fmt(o.normalized_spread_pct)}%</td><td>$${fmt(o.max_size_usd,0)}</td><td>${fmtMs(o.signal_duration_ms)}</td><td>${o.signal_score}/10</td><td><span class="badge badge-${sc}">${o.signal_status}</span></td></tr>`;});
}
function loadOrders(){
  fetch('/orders').then(r=>r.json()).then(orders=>{
    const tbody=document.getElementById('orders-table');const countEl=document.getElementById('orders-count');
    if(!orders||orders.length===0){tbody.innerHTML='<tr><td colspan="7" style="color:var(--muted);text-align:center">Нет исполненных ордеров</td></tr>';countEl.textContent='0 ордеров';return;}
    countEl.textContent=orders.length+' ордеров';
    tbody.innerHTML=orders.slice(0,30).map(o=>{
      const t=new Date(o.time).toLocaleString('ru-RU',{timeZone:'Asia/Jerusalem'});
      const sideColor=o.side==='BUY'?'var(--green)':'var(--red)';const sideText=o.side==='BUY'?'🟢 BUY':'🔴 SELL';
      return`<tr><td style="color:#7b8099;font-size:12px">${t}</td><td>${o.exchange}</td><td><b>${o.symbol}</b></td><td style="color:${sideColor};font-weight:500">${sideText}</td><td>${o.qty.toFixed(4)}</td><td>$${o.price.toFixed(4)}</td><td style="font-weight:500">$${o.total.toFixed(2)}</td></tr>`;
    }).join('');
  }).catch(()=>{document.getElementById('orders-table').innerHTML='<tr><td colspan="7" style="color:var(--red)">Ошибка загрузки</td></tr>';});
}
loadData();setInterval(loadData,10000);
loadOrders();setInterval(loadOrders,120000);
loadBalances();setInterval(loadBalances,60000);
</script></body></html>'''

TRADES_HTML = '''<!DOCTYPE html>
<html lang="ru"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Trade Diary</title>
<style>''' + _CSS + '''</style></head><body>
<div class="header"><h1>Trade Diary</h1><div style="display:flex;gap:12px;align-items:center">''' + _nav('t') + '''</div></div>
<div class="container">
<div class="grid4">
  <div class="metric"><div class="metric-label">Successful trades</div><div class="metric-value green" id="mt">—</div></div>
  <div class="metric"><div class="metric-label">Total arb profit</div><div class="metric-value green" id="mp">—</div></div>
  <div class="metric"><div class="metric-label">Avg profit/trade</div><div class="metric-value" id="ma">—</div></div>
  <div class="metric"><div class="metric-label">Avg latency</div><div class="metric-value" id="ml2">—</div></div>
</div>
<div class="grid3">
  <div class="metric"><div class="metric-label">Volume traded</div><div class="metric-value" id="mv2">—</div></div>
  <div class="metric"><div class="metric-label">Win rate</div><div class="metric-value green" id="mw">—</div></div>
  <div class="metric"><div class="metric-label">Skipped (no funds)</div><div class="metric-value amber" id="mf">—</div></div>
</div>
<div class="section-divider"><span class="section-label">💧 Liquidity Sales (non-arbitrage)</span></div>
<div class="grid4">
  <div class="metric"><div class="metric-label">Total sales</div><div class="metric-value" id="ls-total">—</div><div class="metric-sub" id="ls-types">—</div></div>
  <div class="metric"><div class="metric-label">Total P&amp;L</div><div class="metric-value" id="ls-pnl">—</div><div class="metric-sub" id="ls-avg-pnl">—</div></div>
  <div class="metric"><div class="metric-label">Result breakdown</div><div class="liq-breakdown" id="ls-breakdown">—</div></div>
  <div class="metric"><div class="metric-label">Failed attempts</div><div class="metric-value amber" id="ls-failed">—</div></div>
</div>
<div class="card"><div class="card-title">Liquidity sales — history</div><div id="liq-history"></div></div>
<!-- liq-by-exch removed -->
<div class="section-divider"><span class="section-label">⚡ Arbitrage Trades</span></div>
<div class="card">
  <div class="card-title">By exchange pair</div>
  <table><thead><tr><th>Pair</th><th>Trades</th><th>Volume</th><th>Profit</th></tr></thead><tbody id="pt"></tbody></table>
</div>
<div class="card"><div class="card-title">Trade history</div><div id="tl"></div></div>
</div>
<script>
function fu(n){return(n>=0?'+':'-')+'$'+Math.abs(n).toFixed(4);}
function fuShort(n){return(n>=0?'+':'-')+'$'+Math.abs(n).toFixed(2);}
function typeLabel(t){if(t==='LIQUIDITY_RECOVERY')return'<span class="b bliq">LIQ</span>';if(t==='STUCK_EXIT')return'<span class="b bstuck">STUCK</span>';return t;}
function load(){
  fetch('/trades_data').then(r=>r.json()).then(d=>{
    if(!d)return;
    const s=d.summary||{};
    document.getElementById('mt').textContent=s.success||0;
    const p=s.total_profit||0;const pe=document.getElementById('mp');pe.textContent=fu(p);pe.className='metric-value '+(p>=0?'green':'red');
    const avgEl=document.getElementById('ma');avgEl.textContent=fu(s.avg_profit||0);avgEl.className='metric-value '+((s.avg_profit||0)>=0?'green':'red');
    document.getElementById('ml2').textContent=(s.avg_latency||0).toFixed(0)+'ms';
    document.getElementById('mv2').textContent='$'+(s.total_volume||0).toFixed(2);
    document.getElementById('mf').textContent=s.buy_failed||0;
    const denom=(s.total||0)-(s.buy_failed||0);
    document.getElementById('mw').textContent=denom>0?(((s.success||0)/denom)*100).toFixed(0)+'%':'—';

    const ls=d.liq_summary||{};const noLiq=!ls||ls.total_sales==null;
    if(noLiq){
      document.getElementById('ls-total').textContent='0';
      document.getElementById('ls-types').textContent='No data yet';
      document.getElementById('ls-pnl').textContent='+$0.00';
      document.getElementById('ls-avg-pnl').textContent='avg — ';
      document.getElementById('ls-breakdown').innerHTML='<span style="color:var(--muted)">—</span>';
      document.getElementById('ls-failed').textContent='0';
    }else{
      document.getElementById('ls-total').textContent=ls.success_sales||0;
      document.getElementById('ls-types').textContent=(ls.liq_recovery_cnt||0)+' LIQ · '+(ls.stuck_exit_cnt||0)+' STUCK';
      const pnl=ls.total_pnl||0;const lsPnlEl=document.getElementById('ls-pnl');lsPnlEl.textContent=fuShort(pnl);lsPnlEl.className='metric-value '+(pnl>=0?'green':'red');
      const avgPnl=ls.avg_pnl||0;document.getElementById('ls-avg-pnl').innerHTML='avg <span class="'+(avgPnl>=0?'green':'red')+'">'+fu(avgPnl)+'</span>';
      document.getElementById('ls-breakdown').innerHTML='<div class="liq-row"><span class="green">✅ '+(ls.pnl_positive||0)+' profit</span></div><div class="liq-row"><span style="color:var(--muted)">➖ '+(ls.pnl_zero||0)+' zero</span></div><div class="liq-row"><span class="red">🔴 '+(ls.pnl_negative||0)+' loss</span></div>';
      document.getElementById('ls-failed').textContent=ls.failed_sales||0;
    }

    // Liquidity sales — grouped by day (same style as Trade history)
    const liqSales=d.liq_sales||[];
    if(liqSales.length===0){
      document.getElementById('liq-history').innerHTML='<p style="color:var(--muted);text-align:center;padding:16px">Liquidity sales will appear here when the bot sells crypto to recover stablecoins.</p>';
    }else{
      const liqByDay={};
      liqSales.forEach(r=>{
        const day=new Date(r.timestamp_ms).toLocaleDateString('ru-RU',{timeZone:'Asia/Jerusalem',day:'2-digit',month:'short',year:'numeric'});
        if(!liqByDay[day])liqByDay[day]={rows:[],pnl:0,cnt:0,failed:0};
        liqByDay[day].rows.push(r);
        if(r.status==='SUCCESS'){liqByDay[day].pnl+=(r.pnl_usd||0);liqByDay[day].cnt++;}
        else liqByDay[day].failed++;
      });
      let lh='';
      for(const[day,dd]of Object.entries(liqByDay)){
        const pnlKnown=dd.rows.some(r=>r.status==='SUCCESS'&&r.buy_price>0);
        lh+=`<div class="day-header"><span class="day-title">${day}</span><div class="day-stats"><span>${dd.cnt} sales</span>${dd.failed?`<span style="color:var(--red)">${dd.failed} failed</span>`:''}<span class="${dd.pnl>=0?'green':'red'}" style="font-weight:600">${pnlKnown?fu(dd.pnl):'—'}</span></div></div>`;
        lh+=`<table><thead><tr><th>Time</th><th>Type</th><th>Exchange</th><th>Symbol</th><th>Qty</th><th>Buy $</th><th>Sell $</th><th>Cost</th><th>Recv</th><th>P&L</th><th>P&L %</th><th>Status</th></tr></thead><tbody>`;
        dd.rows.forEach(r=>{
          const tm=new Date(r.timestamp_ms).toLocaleTimeString('ru-RU',{timeZone:'Asia/Jerusalem'});
          const pnl=r.pnl_usd||0;const pnlPct=r.pnl_pct||0;const noBuyPrice=!r.buy_price||r.buy_price===0;
          lh+=`<tr><td style="color:var(--muted)">${tm}</td><td>${typeLabel(r.sale_type)}</td><td>${r.exchange}</td><td><b>${r.symbol}</b></td><td>${(r.qty||0).toFixed(4)}</td><td>${noBuyPrice?'<span style="color:var(--muted)">—</span>':'$'+r.buy_price.toFixed(4)}</td><td>${r.sell_price?'$'+r.sell_price.toFixed(4):'<span style="color:var(--muted)">—</span>'}</td><td>${r.buy_cost_usd?'$'+r.buy_cost_usd.toFixed(4):'—'}</td><td>${r.sell_recv_usd?'$'+r.sell_recv_usd.toFixed(4):'—'}</td><td class="${pnl>0?'green':pnl<0?'red':''}" style="font-weight:500">${noBuyPrice?'<span style="color:var(--muted)">unknown</span>':fu(pnl)}</td><td class="${pnlPct>0?'green':pnlPct<0?'red':''}">${noBuyPrice?'—':(pnlPct>=0?'+':'')+pnlPct.toFixed(3)+'%'}</td><td>${r.status==='SUCCESS'?'<span class="b bok">OK</span>':'<span class="b bfl">FAIL</span>'}</td></tr>`;
        });
        lh+='</tbody></table>';
      }
      document.getElementById('liq-history').innerHTML=lh;
    }

    document.getElementById('pt').innerHTML=(d.by_pair||[]).map(r=>`<tr><td style="font-weight:500">${r.pair}</td><td>${r.success}/${r.total}</td><td>$${(r.volume||0).toFixed(2)}</td><td class="${(r.profit||0)>=0?'green':'red'}" style="font-weight:500">${fu(r.profit||0)}</td></tr>`).join('');

    const trades=d.trades||{};const byDay={};
    // Считаем liq P&L по дням для отображения в Trade history
    const liqPnlByDay={};
    (d.liq_sales||[]).forEach(r=>{
      if(r.status!=='SUCCESS'||!r.buy_price||r.buy_price===0)return;
      const day=new Date(r.timestamp_ms).toLocaleDateString('ru-RU',{timeZone:'Asia/Jerusalem',day:'2-digit',month:'short',year:'numeric'});
      if(!liqPnlByDay[day])liqPnlByDay[day]=0;
      liqPnlByDay[day]+=(r.pnl_usd||0);
    });
    trades.forEach(t=>{
      const day=new Date(t.timestamp_ms).toLocaleDateString('ru-RU',{timeZone:'Asia/Jerusalem',day:'2-digit',month:'short',year:'numeric'});
      if(!byDay[day])byDay[day]={trades:[],profit:0,vol:0,ok:0,fail:0,skip:0};
      byDay[day].trades.push(t);
      if(t.status==='SUCCESS'){byDay[day].profit+=t.profit_usd;byDay[day].vol+=t.trade_size_usd;byDay[day].ok++;}
      else if(t.status==='BUY_FAILED')byDay[day].skip++;
      else byDay[day].fail++;
    });
    let h='';
    for(const[day,dd]of Object.entries(byDay)){
      const liqDay=liqPnlByDay[day];const liqBadge=liqDay!=null?`<span style="color:var(--cyan);font-size:11px">💧 liq ${fu(liqDay)}</span>`:'';
      h+=`<div class="day-header"><span class="day-title">${day}</span><div class="day-stats"><span>${dd.ok}/${dd.ok+dd.fail} trades</span>${dd.skip?`<span style="color:var(--amber)">${dd.skip} skipped</span>`:''}<span>$${dd.vol.toFixed(2)}</span><span class="${dd.profit>=0?'green':'red'}" style="font-weight:600">${fu(dd.profit)}</span>${liqBadge}</div></div>`;
      h+=`<table><thead><tr><th>Time</th><th>Pair</th><th>Symbol</th><th>Qty</th><th>Buy $</th><th>Sell $</th><th>Size</th><th>Profit</th><th>Latency</th><th>Status</th></tr></thead><tbody>`;
      dd.trades.forEach(t=>{
        if(t.status!=='SUCCESS')return;
        const tm=new Date(t.timestamp_ms).toLocaleTimeString('ru-RU',{timeZone:'Asia/Jerusalem'});
        const pair=t.buy_exchange+' → '+t.sell_exchange;
        h+=`<tr><td style="color:var(--muted)">${tm}</td><td>${pair}</td><td><b>${t.symbol}</b></td><td>${t.quantity.toFixed(4)}</td><td>$${t.buy_price.toFixed(4)}</td><td>$${t.sell_price.toFixed(4)}</td><td>$${t.trade_size_usd.toFixed(2)}</td><td class="${t.profit_usd>=0?'green':'red'}" style="font-weight:500">${fu(t.profit_usd)}</td><td>${t.latency_ms}ms</td><td><span class="b bok">OK</span></td></tr>`;
      });
      h+='</tbody></table>';
    }
    document.getElementById('tl').innerHTML=h||'<p style="color:var(--muted);text-align:center;padding:20px">Trades will appear here as the bot executes them.</p>';
  });
}
load();setInterval(load,30000);
</script></body></html>'''

LOGS_HTML = '''<!DOCTYPE html>
<html lang="ru"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Bot Logs</title>
<style>''' + _CSS + '''</style></head><body>
<div class="header"><h1>Bot Logs</h1><div style="display:flex;gap:12px;align-items:center">''' + _nav('l') + '''</div></div>
<div class="container">
<div class="toolbar">
  <button class="fbtn on" onclick="setFilter('',this)">All</button>
  <button class="fbtn" onclick="setFilter('ERROR',this)">Error</button>
  <button class="fbtn" onclick="setFilter('WARN',this)">Warn</button>
  <button class="fbtn" onclick="setFilter('INFO',this)">Info</button>
  <button class="fbtn" onclick="setFilter('EXEC',this)">Exec only</button>
  <input class="search" id="search" placeholder="Search logs..." oninput="debounceSearch()"/>
  <label style="font-size:12px;color:var(--muted);margin-left:8px">Tail lines:
    <select id="tail-lines" onchange="reload()" style="background:var(--bg2);color:var(--text);border:1px solid var(--border);border-radius:6px;padding:4px 8px;font-size:12px;margin-left:4px">
      <option value="500">500</option><option value="1000">1 000</option><option value="2000" selected>2 000</option><option value="5000">5 000</option>
    </select>
  </label>
  <span class="loading" id="status" style="margin-left:8px">—</span>
</div>
<div class="log-wrap" id="lw"></div>
<div class="pagination">
  <button id="btn-first" onclick="goPage(1)" disabled>«</button>
  <button id="btn-prev" onclick="goPage(curPage-1)" disabled>‹</button>
  <span class="pg-info" id="pg-info">—</span>
  <button id="btn-next" onclick="goPage(curPage+1)" disabled>›</button>
  <button id="btn-last" onclick="goPage(totalPages)" disabled>»</button>
</div>
</div>
<script>
let curFilter='',curPage=1,totalPages=1,userScrolled=false,searchTimer=null;
function esc(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
function utcToIsrael(u){if(!u)return'';const d=new Date(u+'Z');return d.toLocaleString('he-IL',{timeZone:'Asia/Jerusalem',hour12:false,day:'2-digit',month:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit'});}
function setFilter(f,btn){curFilter=f;curPage=1;document.querySelectorAll('.toolbar .fbtn').forEach(b=>b.classList.remove('on'));btn.classList.add('on');reload();}
function debounceSearch(){clearTimeout(searchTimer);searchTimer=setTimeout(()=>{curPage=1;reload();},300);}
function goPage(p){if(p<1||p>totalPages)return;curPage=p;reload();}
function buildUrl(){const lines=document.getElementById('tail-lines').value;const q=encodeURIComponent(document.getElementById('search').value);return`/logs_data?lines=${lines}&page=${curPage}&size=200&level=${curFilter}&q=${q}`;}
function reload(){
  document.getElementById('status').textContent='загрузка...';
  fetch(buildUrl()).then(r=>r.json()).then(render).catch(err=>{document.getElementById('lw').innerHTML=`<div style="color:var(--red);padding:20px">${esc(String(err))}</div>`;document.getElementById('status').textContent='ошибка';});
}
function render(d){
  if(d.error){document.getElementById('lw').innerHTML=`<div style="color:var(--red);padding:20px">${esc(d.error)}</div>`;document.getElementById('status').textContent='ошибка';return;}
  totalPages=d.pages||1;curPage=d.page||1;
  document.getElementById('pg-info').textContent=`стр. ${curPage} / ${totalPages}  (${d.total} строк)`;
  document.getElementById('btn-first').disabled=curPage<=1;document.getElementById('btn-prev').disabled=curPage<=1;
  document.getElementById('btn-next').disabled=curPage>=totalPages;document.getElementById('btn-last').disabled=curPage>=totalPages;
  const lines=d.lines||[];
  if(lines.length===0){document.getElementById('lw').innerHTML='<div style="color:var(--muted);text-align:center;padding:40px">Нет строк по фильтру</div>';document.getElementById('status').textContent='0 строк';return;}
  let html='';
  for(const l of lines){const cls=l.level==='ERROR'?'err-line':l.level==='WARN'?'warn-line':'';html+=`<div class="log-line ${cls}"><span class="log-ts">${esc(utcToIsrael(l.ts))}</span><span class="log-lvl lvl-${l.level}">${l.level}</span><span class="log-msg">${esc(l.msg)}</span></div>`;}
  const wrap=document.getElementById('lw');wrap.innerHTML=html;
  document.getElementById('status').textContent=`${lines.length} строк на странице`;
  if(curPage===totalPages&&!userScrolled)wrap.scrollTop=wrap.scrollHeight;
}
document.addEventListener('DOMContentLoaded',()=>{
  const wrap=document.getElementById('lw');
  wrap.addEventListener('scroll',()=>{const atBottom=wrap.scrollHeight-wrap.scrollTop-wrap.clientHeight<50;userScrolled=!atBottom;if(atBottom)userScrolled=false;});
});
reload();setInterval(()=>{if(curPage===totalPages)reload();},15000);
</script></body></html>'''

# ---------------------------------------------------------------------------
# HTTP handler
# ---------------------------------------------------------------------------

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass

    def do_GET(self):
        path = self.path.split('?')[0]
        qs = urllib.parse.parse_qs(self.path[self.path.find('?')+1:] if '?' in self.path else '')

        def _int(key, default):
            try: return int(qs.get(key, [str(default)])[0])
            except: return default
        def _str(key, default=''):
            return qs.get(key, [default])[0]

        if path == '/data':
            body = json.dumps(get_data(), default=str).encode()
        elif path == '/balances':
            body = json.dumps(get_balances(), default=str).encode()
        elif path == '/start_balance':
            balances = get_balances()
            data = get_or_set_start_balance(balances)
            body = json.dumps(data or {}, default=str).encode()
        elif path == '/orders':
            body = json.dumps(get_real_orders(), default=str).encode()
        elif path == '/reset_start_balance':
            if START_BALANCE_FILE.exists():
                START_BALANCE_FILE.unlink()
            body = b'{"reset": true}'
        elif path == '/trades_data':
            body = json.dumps(get_trades_data(), default=str).encode()
        elif path == '/logs_data':
            data = get_logs_data(
                lines  = _int('lines', 500),
                page   = _int('page', 1),
                size   = _int('size', 200),
                level_filter = _str('level').upper() or None,
                q      = _str('q') or None,
            )
            body = json.dumps(data, default=str).encode()
        elif path == '/trades':
            body = TRADES_HTML.encode()
            self._send_html(body); return
        elif path == '/logs':
            body = LOGS_HTML.encode()
            self._send_html(body); return
        else:
            body = HTML.encode()
            self._send_html(body); return

        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', len(body))
        self.end_headers()
        self.wfile.write(body)

    def _send_html(self, body):
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', len(body))
        self.end_headers()
        self.wfile.write(body)


if __name__ == '__main__':
    srv = http.server.HTTPServer(('0.0.0.0', PORT), Handler)
    print(f'Dashboard: http://localhost:{PORT}')
    print(f'База данных: {DB}')
    print('Ctrl+C для остановки')
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        print('\nОстановлен')
