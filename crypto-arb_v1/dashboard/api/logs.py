"""
/logs_data  — tail bot.log with pagination and level filter.

Query params:
  lines   int   how many raw lines to tail (default 500, max 5000)
  page    int   1-based page number (default 1)
  size    int   parsed log entries per page (default 200)
  level   str   filter by level: ERROR | WARN | INFO | DEBUG | EXEC | (empty = all)
  q       str   free-text search (case-insensitive)
"""
import re
from pathlib import Path
from fastapi import APIRouter, Query
from fastapi.responses import JSONResponse

router = APIRouter()

_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
_TS_RE = re.compile(
    r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})\.\d+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s+(.*)"
)
_EXEC_KEYS = ("[EXEC]", "[exec]", "coinbase-exec", "binance-exec", "bitget-exec",
              "[INV-OPT]", "[STUCK]")

LOG_PATH = Path("bot.log")


def _tail_file(path: Path, n: int) -> list[str]:
    """Memory-efficient tail: read last n lines."""
    with open(path, "rb") as f:
        # Seek from end in 64 KB chunks
        chunk = 65536
        lines_found: list[bytes] = []
        remainder = b""
        pos = 0

        f.seek(0, 2)
        file_size = f.tell()
        pos = file_size

        while pos > 0 and len(lines_found) < n:
            read_size = min(chunk, pos)
            pos -= read_size
            f.seek(pos)
            data = f.read(read_size) + remainder
            lines_in_chunk = data.split(b"\n")
            remainder = lines_in_chunk[0]
            lines_found = lines_in_chunk[1:] + lines_found

        if remainder:
            lines_found = [remainder] + lines_found

    tail = lines_found[-n:] if len(lines_found) > n else lines_found
    return [l.decode("utf-8", errors="replace") for l in tail]


def _parse_lines(
    raw_lines: list[str],
    level_filter: str | None = None,
    q: str | None = None,
) -> list[dict]:
    result: list[dict] = []
    for raw in raw_lines:
        raw = _ANSI_RE.sub("", raw).rstrip()
        if not raw:
            continue
        m = _TS_RE.match(raw)
        if m:
            ts_utc, level, msg = m.group(1), m.group(2), m.group(3)
            # Level filter
            if level_filter:
                if level_filter == "EXEC":
                    if not any(k in msg for k in _EXEC_KEYS):
                        continue
                elif level != level_filter:
                    continue
            # Text search
            if q and q.lower() not in msg.lower() and q.lower() not in level.lower():
                continue
            result.append({"ts": ts_utc, "level": level, "msg": msg})
        else:
            # continuation line — append to last entry
            if result:
                result[-1]["msg"] += "\n" + raw
            else:
                if not level_filter and (not q or q.lower() in raw.lower()):
                    result.append({"ts": "", "level": "INFO", "msg": raw})
    return result


@router.get("/logs_data")
async def api_logs_data(
    lines: int = Query(default=500, ge=1, le=5000),
    page: int  = Query(default=1, ge=1),
    size: int  = Query(default=200, ge=10, le=1000),
    level: str = Query(default=""),
    q: str     = Query(default=""),
):
    if not LOG_PATH.exists():
        return JSONResponse(content={"lines": [], "total": 0, "pages": 0,
                                     "error": "bot.log not found"})
    try:
        raw = _tail_file(LOG_PATH, lines)
        parsed = _parse_lines(
            raw,
            level_filter=level.upper() if level else None,
            q=q or None,
        )

        total = len(parsed)
        pages = max(1, (total + size - 1) // size)
        page = min(page, pages)

        start = (page - 1) * size
        end   = start + size
        page_lines = parsed[start:end]

        return JSONResponse(content={
            "lines": page_lines,
            "total": total,
            "page":  page,
            "pages": pages,
            "size":  size,
        })
    except Exception as e:
        return JSONResponse(content={"lines": [], "total": 0, "pages": 0,
                                     "error": str(e)})
