from fastapi import APIRouter
from fastapi.responses import JSONResponse
from exchange.orders import get_real_orders

router = APIRouter()


@router.get("/orders")
async def api_orders():
    return JSONResponse(content=get_real_orders())
