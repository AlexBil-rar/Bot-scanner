# crypto-arb_v1/dashboard

FastAPI rewrite of the original `dashboard.py`.

## Structure

```
dashboard/
├── server.py               # FastAPI app + page routes
├── requirements.txt
├── api/
│   ├── data.py             # GET /data          — opportunities, spreads, stats
│   ├── balances.py         # GET /balances       — exchange balances (60s cache)
│   │                       # GET /start_balance
│   │                       # GET /reset_start_balance
│   ├── orders.py           # GET /orders         — real filled orders (Apr 1+)
│   ├── trades.py           # GET /trades_data    — SQLite trade diary
│   └── logs.py             # GET /logs_data      — tail + pagination
├── exchange/
│   ├── common.py           # read_env, hmac helpers, fetch_json
│   ├── balances.py         # per-exchange balance fetchers
│   └── orders.py           # per-exchange order fetchers
├── templates/
│   ├── base.html           # shared layout / nav
│   ├── index.html          # /  — main dashboard
│   ├── trades.html         # /trades
│   └── logs.html           # /logs
└── static/
    ├── css/main.css        # shared styles
    └── js/
        ├── dashboard.js    # charts, balances, orders, signals
        ├── trades.js       # trade diary logic
        └── logs.js         # logs tail, pagination, filter, search

```

## Run

```bash
pip install -r requirements.txt

# from the dashboard/ directory (same level as arb_data.db and bot.log):
python3 server.py
# or with auto-reload for dev:
uvicorn server:app --host 0.0.0.0 --port 8765 --reload
```

## Logs endpoint — query params

| param   | default | description                                 |
|---------|---------|---------------------------------------------|
| `lines` | 500     | how many raw lines to tail (max 5000)       |
| `page`  | 1       | 1-based page number                         |
| `size`  | 200     | parsed entries per page (10–1000)           |
| `level` | (all)   | ERROR / WARN / INFO / DEBUG / EXEC          |
| `q`     | (all)   | free-text search (case-insensitive)         |

Example: `/logs_data?lines=2000&page=2&size=200&level=ERROR`

## Notes

- `server.py` must be started from the directory containing `arb_data.db`, `bot.log`, and `.env`.
- Bybit is included in balances (read-only, not in opportunity filters).
- Balance cache TTL is 60 s; start balance is stored in `start_balance.json`.
