"""Real multipart DELETE settlement under AnyIO level cancellation."""

import asyncio

import anyio
import pytest

from app.config import settings
from app.remote_storage import S3Storage
from tests.test_s3_protocol import s3_service  # noqa: F401


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["edge", "level"])
async def test_s3_abort_settles_once_without_cancellation_spin(
    s3_service, tmp_path, monkeypatch, mode
):
    backend, state = s3_service
    monkeypatch.setattr(settings, "S3_MULTIPART_THRESHOLD_BYTES", 5 * 1024 * 1024)
    monkeypatch.setattr(S3Storage, "MULTIPART_CLEANUP_TIMEOUT", 2)
    source = tmp_path / "owned.tar"
    source.write_bytes(b"x" * (6 * 1024 * 1024))
    state.mode, state.abort_mode = "hold", "hold"
    scopes = []
    propagated = asyncio.Event()

    async def upload():
        with anyio.CancelScope() as scope:
            scopes.append(scope)
            try:
                await backend.upload(source, "owned.tar")
            except asyncio.CancelledError:
                propagated.set()
                raise
            pytest.fail("Cancelled multipart upload reported a result")

    task = asyncio.create_task(upload())
    try:
        await asyncio.wait_for(state.entered.wait(), 2)
        if mode == "level":
            scopes[0].cancel()
        else:
            task.cancel()
        await asyncio.wait_for(state.abort_entered.wait(), 2)
        for _ in range(20):
            await asyncio.sleep(0)
        task.cancel()
        for _ in range(20):
            await asyncio.sleep(0)
        deliveries = task.cancelling()
        assert not task.done() and state.uploads, "Returned before actual HTTP abort"
    finally:
        state.release.set()
        state.abort_release.set()
        await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 3)
    assert deliveries <= 4, f"Hot S3 cleanup cancellation loop: {deliveries}"
    assert propagated.is_set()
    assert not state.uploads and not state.objects
    assert sum(method == "DELETE" for method, *_ in state.events) == 1
