"""Native HTTP timeout and in-flight disk-write cleanup under repeated cancellation."""

import asyncio
import os
import threading

import aioboto3
from botocore.config import Config
import pytest

from tests import test_python_io_performance

download_service = test_python_io_performance.download_service


@pytest.mark.asyncio
async def test_s3_native_read_timeout_closes_body_and_preserves_published_file(
    download_service, tmp_path
):
    storage, state, _ = download_service
    state.mode = "hold"
    destination = tmp_path / "archive"
    destination.write_bytes(b"prior")
    async with aioboto3.Session().client(
        "s3",
        endpoint_url=storage.config.s3_endpoint_url,
        aws_access_key_id="owned",
        aws_secret_access_key="owned",
        config=Config(read_timeout=0.05, retries={"max_attempts": 0}),
    ) as client:
        storage._client = client
        try:
            assert not await storage.download("archive", destination)
            assert destination.read_bytes() == b"prior"
            assert set(tmp_path.iterdir()) == {destination}
        finally:
            storage._client = None
            state.release.set()


@pytest.mark.asyncio
async def test_s3_cancellation_joins_real_disk_writer_before_unlink(
    download_service, tmp_path, monkeypatch
):
    storage, state, payload = download_service
    destination = tmp_path / "archive"
    destination.write_bytes(b"prior")
    entered, release = threading.Event(), threading.Event()
    original = os.fdopen

    class Writer:
        def __init__(self, file):
            self.file = file

        def __enter__(self):
            return self

        def __exit__(self, *args):
            self.file.close()

        def write(self, chunk):
            entered.set()
            assert release.wait(5)
            return self.file.write(chunk)

    monkeypatch.setattr(
        os, "fdopen", lambda *args, **kwargs: Writer(original(*args, **kwargs))
    )
    operation = asyncio.create_task(storage.download("archive", destination))
    try:
        async with asyncio.timeout(5):
            while not entered.is_set():
                await asyncio.sleep(0.001)
        operation.cancel()
        done, _ = await asyncio.wait([operation], timeout=0.05)
        assert not done
        assert destination.read_bytes() == b"prior"
        assert len(list(tmp_path.iterdir())) == 2
        operation.cancel()
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await operation
        assert destination.read_bytes() == b"prior"
        assert set(tmp_path.iterdir()) == {destination}
    finally:
        release.set()
        await asyncio.gather(operation, return_exceptions=True)
