"""Run directly after a clean runtime-only install (no pytest required)."""

import asyncio
import os
import tempfile
from pathlib import Path


async def check_runtime():
    # Set paths before importing the application; never touch an installed DB.
    with tempfile.TemporaryDirectory(prefix="dockervault-runtime-") as directory:
        os.environ["DATABASE_URL"] = f"sqlite+aiosqlite:///{directory}/backup.db"
        os.environ["BACKUP_BASE_PATH"] = directory
        from sqlalchemy import text

        from app.database import async_session, engine, init_db
        from app.main import app

        assert app.title
        try:
            await init_db()
            await init_db()
            async with async_session() as session:
                assert (await session.execute(text("SELECT 1"))).scalar_one() == 1
            assert Path(directory, "backup.db").is_file()
        finally:
            await engine.dispose()
    print("Runtime import and repeated SQLite initialization passed")


if __name__ == "__main__":
    asyncio.run(check_runtime())
