"""Owned TLS browser fixture: real app plus a foreign same-site origin."""

import json
import os
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ssl
import subprocess
import threading

import uvicorn

root = Path(os.environ["DV_BROWSER_ROOT"]).resolve()
root.mkdir(parents=True, exist_ok=True)
key, certificate = root / "tls.key", root / "tls.crt"
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
        str(key),
        "-out",
        str(certificate),
    ],
    check=True,
)

from app.main import app
from starlette.staticfiles import StaticFiles

app.mount(
    "/",
    StaticFiles(directory="/usr/share/nginx/html", html=True),
    name="owned-browser-assets",
)


class ForeignOrigin(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<!doctype html><title>Owned foreign origin fixture</title>")

    def log_message(self, format, *args):
        pass


foreign = ThreadingHTTPServer(("127.0.0.1", 8444), ForeignOrigin)
tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
tls.load_cert_chain(certificate, key)
foreign.socket = tls.wrap_socket(foreign.socket, server_side=True)
threading.Thread(target=foreign.serve_forever, daemon=True).start()


class EvidenceBoundary:
    """Record cookie presence and policy result, never cookie/token contents."""

    def __init__(self, application):
        self.application = application
        self.rows = []

    async def __call__(self, scope, receive, send):
        if scope["type"] != "websocket":
            return await self.application(scope, receive, send)
        headers = {key.lower(): value for key, value in scope["headers"]}
        row = {
            "origin": headers.get(b"origin", b"").decode(),
            "cookie_present": b"session_token=" in headers.get(b"cookie", b""),
            "accepted": False,
            "denial_code": None,
        }
        self.rows.append(row)

        async def recorded(message):
            if message["type"] == "websocket.accept":
                row["accepted"] = True
            if message["type"] == "websocket.close" and not row["accepted"]:
                row["denial_code"] = message["code"]
            (root / "evidence.json").write_text(json.dumps(self.rows))
            await send(message)

        await self.application(scope, receive, recorded)


uvicorn.run(
    EvidenceBoundary(app),
    host="127.0.0.1",
    port=8443,
    ssl_keyfile=str(key),
    ssl_certfile=str(certificate),
    access_log=False,
    log_level="warning",
)
