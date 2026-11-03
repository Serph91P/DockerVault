"""Isolated product HTTP/browser fixture; no Docker socket or auth/API mocks."""

import os
from pathlib import Path
import subprocess

import uvicorn
from starlette.staticfiles import StaticFiles

root = Path("/app/data/ux181")
root.mkdir(parents=True, exist_ok=True)
source = root / "source"
(source / "nested").mkdir(parents=True, exist_ok=True)
(source / "nested" / "payload.txt").write_bytes(
    b"DockerVault synthetic recovery bytes\n"
)
for index in range(2003):
    (source / f"file-{index:04d}.txt").write_text(f"synthetic {index}\n")
subprocess.run(
    [
        "openssl",
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "1",
        "-subj",
        "/CN=localhost",
        "-keyout",
        str(root / "tls.key"),
        "-out",
        str(root / "tls.crt"),
    ],
    check=True,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
for path in (root, source, *source.rglob("*")):
    os.chown(path, 1000, 1000)

from app.main import app


class ProductAndAssets:
    """Mirror nginx: public frontend assets, untouched authenticated API/WS app."""

    def __init__(self):
        self.assets = StaticFiles(directory="/usr/share/nginx/html", html=True)

    async def __call__(self, scope, receive, send):
        if (
            scope["type"] != "http"
            or scope["path"].startswith(("/api/", "/ws/"))
            or scope["path"] == "/health"
        ):
            return await app(scope, receive, send)
        return await self.assets(scope, receive, send)


uvicorn.run(
    ProductAndAssets(),
    host="127.0.0.1",
    port=8443,
    ssl_keyfile=str(root / "tls.key"),
    ssl_certfile=str(root / "tls.crt"),
    access_log=False,
    log_level="warning",
)
