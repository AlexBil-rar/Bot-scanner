"""
Fetch real filled orders from all exchanges since April 1, 2026.
"""
import time
import json
import urllib.request
from .common import read_env, fetch_json, hmac_sha256_hex, hmac_sha256_b64, now_ms

SYMBOLS = ["XRPUSDT", "SOLUSDT", "ETHUSDT", "BTCUSDT"]
START_TS = int(time.mktime(time.strptime("2026-04-01", "%Y-%m-%d"))) * 1000


def _binance_orders(env: dict) -> list[dict]:
    orders = []
    api_key = env.get("BINANCE_API_KEY", "")
    api_secret = env.get("BINANCE_API_SECRET", "")
    if not api_key:
        return orders
    try:
        ts = now_ms()
        for symbol in SYMBOLS:
            params = f"symbol={symbol}&startTime={START_TS}&limit=50&timestamp={ts}&recvWindow=5000"
            sig = hmac_sha256_hex(api_secret, params)
            url = f"https://api.binance.com/api/v3/allOrders?{params}&signature={sig}"
            data = fetch_json(url, headers={"X-MBX-APIKEY": api_key})
            for o in data:
                if o.get("status") != "FILLED":
                    continue
                exec_qty = float(o["executedQty"])
                price = (
                    float(o["price"])
                    if float(o["price"]) > 0
                    else float(o.get("cummulativeQuoteQty", 0)) / max(exec_qty, 1e-9)
                )
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
    return orders


def _bitget_orders(env: dict) -> list[dict]:
    orders = []
    api_key = env.get("BITGET_API_KEY", "")
    api_secret = env.get("BITGET_API_SECRET", "")
    passphrase = env.get("BITGET_PASSPHRASE", "")
    if not api_key:
        return orders
    try:
        for symbol in SYMBOLS:
            ts = str(now_ms())
            path = f"/api/v2/spot/trade/history-orders?symbol={symbol}&limit=50"
            sig = hmac_sha256_b64(api_secret, ts + "GET" + path)
            data = fetch_json(
                f"https://api.bitget.com{path}",
                headers={
                    "ACCESS-KEY": api_key,
                    "ACCESS-SIGN": sig,
                    "ACCESS-TIMESTAMP": ts,
                    "ACCESS-PASSPHRASE": passphrase,
                    "Content-Type": "application/json",
                },
            )
            for o in data.get("data") or []:
                try:
                    ts_order = int(o.get("cTime", 0))
                    if ts_order < START_TS:
                        continue
                    if o.get("status") not in ("filled", "full_fill"):
                        continue
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
                except Exception:
                    pass
    except Exception:
        pass
    return orders


def _coinbase_orders(env: dict) -> list[dict]:
    orders = []
    api_key = env.get("COINBASE_API_KEY", "")
    api_secret_path = env.get("COINBASE_API_SECRET", "")
    if not api_key:
        return orders
    try:
        from coinbase.rest import RESTClient
        from pathlib import Path

        secret = (
            Path(api_secret_path).read_text().strip()
            if Path(api_secret_path).exists()
            else api_secret_path
        )
        client = RESTClient(api_key=api_key, api_secret=secret)
        result = client.list_orders(order_status=["FILLED"], limit=50)
        for o in result.orders:
            try:
                ts_order = (
                    int(time.mktime(time.strptime(o.created_time[:19], "%Y-%m-%dT%H:%M:%S")))
                    * 1000
                )
                if ts_order < START_TS:
                    continue
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
            except Exception:
                pass
    except Exception:
        pass
    return orders


def get_real_orders() -> list[dict]:
    env = read_env()
    orders: list[dict] = []
    orders.extend(_binance_orders(env))
    orders.extend(_bitget_orders(env))
    orders.extend(_coinbase_orders(env))
    orders.sort(key=lambda x: x["time"], reverse=True)
    return orders
