"""The real codec process must exit before key-file cleanup on cancellation."""

import asyncio
from pathlib import Path
import sys

import pytest

from app.encryption import _crypt_file
from tests.test_level_cancellation import cancel_while_held


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["edge", "level"])
async def test_codec_cancellation_joins_process_without_spin(
    tmp_path, monkeypatch, mode
):
    ready, stopped, release_path = (
        tmp_path / name for name in ("ready", "stopped", "release")
    )
    entered = asyncio.Event()
    processes, keys, key_existed = [], [], []
    original = asyncio.create_subprocess_exec
    script = (
        "import signal,time; from pathlib import Path; "
        f"signal.signal(signal.SIGTERM, lambda *_: Path({str(stopped)!r}).touch()); "
        f"Path({str(ready)!r}).touch(); "
        f"exec({('while not Path(' + repr(str(release_path)) + ').exists(): time.sleep(0.01)')!r})"
    )

    async def start(*args, **kwargs):
        keys.append(Path(args[args.index("--key-material-file") + 1]))
        process = await original(sys.executable, "-c", script, **kwargs)
        processes.append(process)
        while not ready.exists():
            await asyncio.sleep(0.001)
        entered.set()
        return process

    class Release:
        def set(self):
            key_existed.append(keys[0].exists())
            release_path.touch()

    monkeypatch.setattr(asyncio, "create_subprocess_exec", start)

    async def operation():
        await _crypt_file(
            tmp_path / "source", tmp_path / "output", b"x" * 32, decrypt=False
        )

    await cancel_while_held(operation, entered, Release(), mode)
    assert stopped.exists(), "The codec process never received its abort signal"
    assert key_existed == [True], (
        "Key material was removed while the codec still owned it"
    )
    assert processes[0].returncode is not None
    assert not keys[0].exists()
