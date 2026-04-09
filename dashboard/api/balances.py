import time
import json
from pathlib import Path
from fastapi import APIRouter
from fastapi.responses import JSONResponse

from exchange.common import read_env
from exchange.balances import (
    get_coinbase_balance,
    get_binance_balance,
    get_bitget_balance,
    get_bybit_balance,
)

router = APIRouter()

START_BALANCE_FILE = "start_balance.json"

# ---- in-process cache (60s TTL) -----------------------------------------
_cache: dict = {"ts": 0, "data": None}


def get_balances() -> dict:
    global _cache
    now = time.time()
    if now - _cache["ts"] < 60 and _cache["data"]:
        return _cache["data"]

    env = read_env()
    result = {
        "coinbase": get_coinbase_balance(
            env.get("COINBASE_API_KEY", ""),
            env.get("COINBASE_API_SECRET", ""),
        ),
        "binance": get_binance_balance(
            env.get("BINANCE_API_KEY", ""),
            env.get("BINANCE_API_SECRET", ""),
        ),
        "bitget": get_bitget_balance(
            env.get("BITGET_API_KEY", ""),
            env.get("BITGET_API_SECRET", ""),
            env.get("BITGET_PASSPHRASE", ""),
        ),
        "bybit": get_bybit_balance(
            env.get("BYBIT_API_KEY", ""),
            env.get("BYBIT_API_SECRET", ""),
        ),
        "updated_at": int(now),
    }
    _cache = {"ts": now, "data": result}
    return result


def get_or_set_start_balance(current: dict) -> dict | None:
    p = Path(START_BALANCE_FILE)
    if p.exists():
        try:
            return json.loads(p.read_text())
        except Exception:
            pass
    # First run — snapshot current as start
    keys = ["coinbase", "binance", "bitget", "bybit"]
    if current and all(current.get(k) is not None for k in keys):
        total = sum(current.get(k, 0) or 0 for k in keys)
        data = {**current, "total": total, "saved_at": int(time.time())}
        p.write_text(json.dumps(data))
        return data
    return None


# ---- routes -----------------------------------------------------------------

@router.get("/balances")
async def api_balances():
    return JSONResponse(content=get_balances())


@router.get("/start_balance")
async def api_start_balance():
    balances = get_balances()
    data = get_or_set_start_balance(balances)
    return JSONResponse(content=data or {})


@router.get("/reset_start_balance")
async def api_reset_start_balance():
    p = Path(START_BALANCE_FILE)
    if p.exists():
        p.unlink()
    return JSONResponse(content={"reset": True})
