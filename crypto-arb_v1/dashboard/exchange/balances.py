"""
Per-exchange balance fetchers.
Each returns float (USDT equivalent) or None on error.
"""
import time
from pathlib import Path
from .common import fetch_json, hmac_sha256_hex, hmac_sha256_b64, now_ms


# ---------------------------------------------------------------------------
# Coinbase
# ---------------------------------------------------------------------------
def get_coinbase_balance(api_key: str, api_secret: str) -> float | None:
    try:
        from coinbase.rest import RESTClient

        if api_secret.startswith("/") and Path(api_secret).exists():
            api_secret = Path(api_secret).read_text().strip()
        else:
            api_secret = api_secret.replace("\\n", "\n")

        client = RESTClient(api_key=api_key, api_secret=api_secret)
        accounts = client.get_accounts()

        total_usdc = 0.0
        crypto_holdings: dict[str, float] = {}

        for acc in accounts.accounts:
            val = float(acc.available_balance["value"] or 0)
            if val <= 0:
                continue
            if acc.currency in ("USD", "USDC"):
                total_usdc += val
            elif acc.currency in ("XRP", "BTC", "ETH", "SOL", "BNB"):
                crypto_holdings[acc.currency] = val

        if crypto_holdings:
            try:
                products = client.get_best_bid_ask(
                    product_ids=[f"{c}-USDC" for c in crypto_holdings]
                )
                prices: dict[str, float] = {}
                for p in products.pricebooks:
                    symbol = p.product_id.split("-")[0]
                    if p.bids:
                        prices[symbol] = float(p.bids[0].price)
                for symbol, qty in crypto_holdings.items():
                    if symbol in prices:
                        total_usdc += qty * prices[symbol]
            except Exception:
                pass

        return round(total_usdc, 2)
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Binance
# ---------------------------------------------------------------------------
def get_binance_balance(api_key: str, api_secret: str) -> float | None:
    try:
        ts = now_ms()
        params = f"timestamp={ts}&recvWindow=5000"
        sig = hmac_sha256_hex(api_secret, params)
        url = f"https://api.binance.com/api/v3/account?{params}&signature={sig}"
        data = fetch_json(url, headers={"X-MBX-APIKEY": api_key})

        prices_data = fetch_json("https://api.binance.com/api/v3/ticker/price")
        prices = {p["symbol"]: float(p["price"]) for p in prices_data}

        total = 0.0
        for bal in data.get("balances", []):
            asset = bal["asset"]
            free = float(bal["free"])
            if free <= 0:
                continue
            if asset in ("USDT", "BUSD"):
                total += free
            elif asset + "USDT" in prices:
                total += free * prices[asset + "USDT"]

        return round(total, 2)
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Bitget
# ---------------------------------------------------------------------------
def get_bitget_balance(api_key: str, api_secret: str, passphrase: str) -> float | None:
    try:
        ts = str(now_ms())
        path = "/api/v2/spot/account/assets"
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

        prices_raw = fetch_json("https://api.bitget.com/api/v2/spot/market/tickers")
        prices = {p["symbol"]: float(p["lastPr"]) for p in prices_raw.get("data", [])}

        total = 0.0
        for asset in data.get("data", []):
            coin = asset.get("coin", "")
            available = float(asset.get("available", 0) or 0)
            if available <= 0:
                continue
            if coin in ("USDT", "USDC"):
                total += available
            elif coin + "USDT" in prices:
                total += available * prices[coin + "USDT"]

        return round(total, 2)
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Bybit
# ---------------------------------------------------------------------------
def get_bybit_balance(api_key: str, api_secret: str) -> float | None:
    try:
        ts = str(now_ms())
        recv_window = "5000"
        params = "accountType=UNIFIED"
        sign_str = ts + api_key + recv_window + params
        sig = hmac_sha256_hex(api_secret, sign_str)

        url = f"https://api.bybit.com/v5/account/wallet-balance?{params}"
        data = fetch_json(
            url,
            headers={
                "X-BAPI-API-KEY": api_key,
                "X-BAPI-SIGN": sig,
                "X-BAPI-TIMESTAMP": ts,
                "X-BAPI-RECV-WINDOW": recv_window,
            },
        )

        if data.get("retCode") != 0:
            return None

        total = 0.0
        for account in data.get("result", {}).get("list", []):
            for coin in account.get("coin", []):
                total += float(coin.get("usdValue", 0) or 0)

        return round(total, 2)
    except Exception:
        return None
