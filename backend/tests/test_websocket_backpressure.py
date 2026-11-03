"""Observability must not govern backup execution or hold connection locks."""

import asyncio
import json
from pathlib import Path
from unittest.mock import patch

import pytest

from app import websocket as ws
from app.backup_engine import BackupEngine
from app.database import Backup, BackupTarget


class Socket:
    def __init__(self, stalled=False):
        self.stalled = stalled
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        self.terminal = asyncio.Event()
        self.messages = []
        self.closed = []
        self.closed_event = asyncio.Event()

    async def accept(self):
        pass

    async def send_text(self, text):
        self.started.set()
        if self.stalled:
            await self.release.wait()
        message = json.loads(text)
        self.messages.append(message)
        if message.get("progress") == 100 or message.get("sequence") == 999:
            self.terminal.set()

    async def send_json(self, message):
        await self.send_text(json.dumps(message))

    async def close(self, code=1000, reason=""):
        self.closed.append((code, reason))
        self.closed_event.set()


@pytest.mark.asyncio
async def test_stalled_socket_cannot_delay_actual_backup_or_healthy_delivery(
    async_client, test_db, tmp_path
):
    source = tmp_path / "source"
    source.mkdir()
    (source / "payload").write_bytes(b"actual synthetic backup bytes")
    async with test_db() as session:
        target = BackupTarget(
            name="owned",
            target_type="path",
            host_path=str(source),
            stop_container=False,
        )
        session.add(target)
        await session.flush()
        backup = Backup(target_id=target.id)
        session.add(backup)
        await session.commit()
        backup_id = backup.id
    manager = ws.ConnectionManager()
    slow, healthy = Socket(stalled=True), Socket()
    await manager.connect(slow)
    await manager.connect(healthy)
    engine = BackupEngine()
    engine.add_progress_callback(ws.backup_progress_callback)
    with (
        patch.object(ws, "manager", manager),
        patch("app.backup_engine.settings.BACKUP_BASE_PATH", str(tmp_path / "backups")),
        patch.object(engine, "_sudo_available", return_value=False),
    ):
        task = asyncio.create_task(engine.run_backup(backup_id, skip_validation=True))
        try:
            await asyncio.wait_for(slow.started.wait(), 5)
            done, _ = await asyncio.wait([task], timeout=2)
            assert done, "stalled notification blocked the backup lifecycle"
            assert task.result() is True
            await asyncio.wait_for(healthy.terminal.wait(), 2)
            response = await async_client.get(f"/api/v1/backups/{backup_id}")
            assert response.status_code == 200
            assert response.json()["status"] == "completed"
            assert Path(response.json()["file_path"]).is_file()
            assert any(
                m.get("type") == "backup_progress" and m.get("progress") == 100
                for m in healthy.messages
            )
        finally:
            slow.release.set()
            await asyncio.gather(task, return_exceptions=True)
            await manager.disconnect(slow)
            await manager.disconnect(healthy)


@pytest.mark.asyncio
async def test_sustained_broadcast_is_bounded_and_disconnect_remains_responsive():
    manager = ws.ConnectionManager()
    slow, healthy = Socket(stalled=True), Socket()
    await manager.connect(slow)
    await manager.connect(healthy)

    async def publish():
        for sequence in range(1000):
            await manager.broadcast({"type": "backup_progress", "sequence": sequence})

    publisher = asyncio.create_task(publish())
    disconnect = None
    try:
        await asyncio.wait_for(slow.started.wait(), 5)
        done, _ = await asyncio.wait([publisher], timeout=0.2)
        assert done, "publisher waited for a network send"
        assert manager._queues[slow].qsize() <= manager.QUEUE_SIZE
        await asyncio.wait_for(healthy.terminal.wait(), 2)
        disconnect = asyncio.create_task(manager.disconnect(slow))
        done, _ = await asyncio.wait([disconnect], timeout=0.2)
        assert done, "disconnect waited behind a network-held global lock"
    finally:
        slow.release.set()
        await asyncio.gather(
            *(t for t in (publisher, disconnect) if t), return_exceptions=True
        )
        await manager.disconnect(slow)
        await manager.disconnect(healthy)


@pytest.mark.asyncio
async def test_stalled_writer_times_out_without_removing_healthy_clients():
    manager = ws.ConnectionManager()
    manager.SEND_TIMEOUT = 0.02
    slow, healthy = Socket(stalled=True), Socket()
    await manager.connect(slow)
    await manager.connect(healthy)
    try:
        await manager.broadcast({"type": "backup_progress", "progress": 100})
        await asyncio.wait_for(slow.closed_event.wait(), 2)
        await asyncio.wait_for(healthy.terminal.wait(), 2)
        assert slow.closed[0][0] == 1013
        assert slow not in manager.active_connections
        assert healthy in manager.active_connections
    finally:
        await manager.shutdown()
    assert not manager._owned and not manager._queues and not manager._senders


@pytest.mark.asyncio
@pytest.mark.parametrize("stalled_close", [False, True])
async def test_shutdown_drains_owned_notification_tasks_even_with_stalled_close(
    stalled_close,
):
    manager = ws.ConnectionManager()
    manager.SEND_TIMEOUT = 0.02
    slow = Socket(stalled=True)
    if stalled_close:

        async def close(**kwargs):
            await asyncio.Event().wait()

        slow.close = close
    await manager.connect(slow)
    await manager.broadcast({"type": "backup_progress", "progress": 0})
    await asyncio.wait_for(slow.started.wait(), 2)
    await asyncio.wait_for(manager.shutdown(), 2)
    assert not manager.active_connections
    assert not manager._owned and not manager._queues and not manager._senders


@pytest.mark.asyncio
async def test_control_frame_survives_progress_coalescing():
    manager = ws.ConnectionManager()
    client = Socket()
    await manager.connect(client)
    try:
        await manager.send_to_client(
            client, {"type": "connected", "message": "Connected to backup manager"}
        )
        for sequence in range(1000):
            await manager.broadcast({"type": "backup_progress", "sequence": sequence})
        await asyncio.wait_for(client.terminal.wait(), 2)
        assert client.messages[0]["type"] == "connected"
        assert client.messages[-1]["sequence"] == 999
    finally:
        await manager.shutdown()


@pytest.mark.asyncio
async def test_control_flood_disconnects_once_without_unbounded_dispatch_tasks():
    manager = ws.ConnectionManager()
    client = Socket(stalled=True)
    await manager.connect(client)
    try:
        for _ in range(1000):
            await manager.send_to_client(client, {"type": "pong"})
        assert len(manager._owned) <= 2
        await asyncio.wait_for(client.closed_event.wait(), 2)
        assert len(client.closed) == 1 and client.closed[0][0] == 1013
    finally:
        await manager.shutdown()
