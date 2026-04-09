import hmac
import hashlib
import base64
import time
import json
import urllib.request
from pathlib import Path


from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent
ROOT_DIR = BASE_DIR.parent

def read_env() -> dict:
    env = {}
    for f in [ROOT_DIR / ".env", ROOT_DIR / ".env.example"]:
        if f.exists():
            for line in f.read_text().splitlines():
                line = line.strip()
                if "=" in line and not line.startswith("#"):
                    k, v = line.split("=", 1)
                    env[k.strip()] = v.strip()
            break
    return env

def fetch_json(url: str, headers: dict = None, timeout: int = 6) -> dict:
    req = urllib.request.Request(url, headers=headers or {})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def hmac_sha256_hex(secret: str, message: str) -> str:
    return hmac.new(secret.encode(), message.encode(), hashlib.sha256).hexdigest()


def hmac_sha256_b64(secret: str, message: str) -> str:
    return base64.b64encode(
        hmac.new(secret.encode(), message.encode(), hashlib.sha256).digest()
    ).decode()


def now_ms() -> int:
    return int(time.time() * 1000)


def now_s() -> int:
    return int(time.time())
