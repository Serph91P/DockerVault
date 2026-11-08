"""Isolated source-runtime fixture for #189; not a canonical Docker image build.

Usage: python tests/runtime_storage_workspace_local.py OWNED_ROOT FRONTEND_DIST
No production socket, bind mount, data or authentication bypass.
"""

import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import sys
import threading

root = Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=True)
dist = Path(sys.argv[2]).resolve()
os.environ.update(
    DATABASE_URL=f"sqlite+aiosqlite:///{root / 'backup.db'}",
    BACKUP_BASE_PATH=str(root / "backups"),
    DOCKER_SOCKET=str(root / "absent.sock"),
    COOKIE_SECURE="false",
    ALLOWED_SSH_KEY_DIRS=str(root / "ssh_keys"),
    KOMODO_ENABLED="false",
)
(root / "backups" / "nested").mkdir(parents=True, exist_ok=True)
(root / "backups" / "nested" / "payload.txt").write_bytes(
    b"exact synthetic storage bytes\x00\xff"
)
(root / "backups" / "nested" / "remove.txt").write_bytes(b"remove synthetic bytes")
(root / "backups" / "nested" / "keep.txt").write_bytes(b"preserved bytes")
(root / "backups" / ".backup_test").write_bytes(b"legacy name must survive")
from app import credential_encryption
from app.api import storage

credential_encryption._KEY_FILE_PATH = root / ".credential_key"
storage._SSH_KEYS_DIR = root / "ssh_keys"
from app.main import app
import uvicorn


class Frontend(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_request(self):
        if self.path.startswith("/api/") or self.path == "/health":
            connection = http.client.HTTPConnection("127.0.0.1", 8000, timeout=60)
            size = int(self.headers.get("Content-Length", "0"))
            connection.request(
                self.command,
                self.path,
                self.rfile.read(size) if size else None,
                {
                    key: value
                    for key, value in self.headers.items()
                    if key.lower() not in ("host", "connection")
                },
            )
            response = connection.getresponse()
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in ("connection", "transfer-encoding"):
                    self.send_header(key, value)
            self.end_headers()
            while data := response.read(65536):
                self.wfile.write(data)
            connection.close()
            return
        path = dist / self.path.split("?", 1)[0].lstrip("/")
        if not path.is_file() or dist not in path.resolve().parents:
            path = dist / "index.html"
        content = path.read_bytes()
        self.send_response(200)
        self.send_header(
            "Content-Type",
            "application/javascript"
            if path.suffix == ".js"
            else "text/css"
            if path.suffix == ".css"
            else "text/html",
        )
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)

    do_GET = do_POST = do_PUT = do_DELETE = do_request


frontend = ThreadingHTTPServer(("127.0.0.1", 5173), Frontend)
threading.Thread(target=frontend.serve_forever, daemon=True).start()
try:
    uvicorn.run(app, host="127.0.0.1", port=8000)
finally:
    frontend.shutdown()
