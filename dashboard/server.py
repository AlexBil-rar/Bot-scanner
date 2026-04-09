#!/usr/bin/env python3
"""
Crypto Arb Dashboard — FastAPI version
Запуск: uvicorn server:app --host 0.0.0.0 --port 8765 --reload
или:    python3 server.py
"""
import uvicorn
from fastapi import FastAPI
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from fastapi.requests import Request
from fastapi.responses import HTMLResponse

from api.data import router as data_router
from api.balances import router as balances_router
from api.orders import router as orders_router
from api.trades import router as trades_router
from api.logs import router as logs_router

app = FastAPI(title="Crypto Arb Dashboard", docs_url=None, redoc_url=None)

app.mount("/static", StaticFiles(directory="static"), name="static")
templates = Jinja2Templates(directory="templates")

app.include_router(data_router)
app.include_router(balances_router)
app.include_router(orders_router)
app.include_router(trades_router)
app.include_router(logs_router)


@app.get("/", response_class=HTMLResponse)
async def index(request: Request):
    return templates.TemplateResponse("index.html", {"request": request})


@app.get("/trades", response_class=HTMLResponse)
async def trades_page(request: Request):
    return templates.TemplateResponse("trades.html", {"request": request})


@app.get("/logs", response_class=HTMLResponse)
async def logs_page(request: Request):
    return templates.TemplateResponse("logs.html", {"request": request})


if __name__ == "__main__":
    uvicorn.run("server:app", host="0.0.0.0", port=8765, reload=False)
