import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import router


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("GET", "/komodo/status"),
        ("POST", "/komodo/test"),
        ("POST", "/komodo/container-action"),
        ("GET", "/komodo/container/example"),
        ("GET", "/settings/komodo"),
        ("PUT", "/settings/komodo"),
        ("POST", "/settings/komodo/test"),
    ],
)
def test_retired_integration_does_not_register_routes(method, path):
    # Mount just the production router; no startup, scheduler or provider I/O.
    app = FastAPI()
    app.include_router(router, prefix="/api/v1")
    client = TestClient(app)
    assert client.request(method, "/api/v1" + path).status_code == 404
    assert "/api/v1/settings/storage" in client.get("/openapi.json").json()["paths"]
