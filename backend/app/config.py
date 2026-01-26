"""
Configuration settings for the backup manager.
"""

import logging

from pydantic_settings import BaseSettings

logger = logging.getLogger(__name__)


class Settings(BaseSettings):
    """Application settings."""

    # Database - fixed path inside container
    DATABASE_URL: str = "sqlite+aiosqlite:///./data/backups.db"

    # Docker - fixed socket path
    DOCKER_SOCKET: str = "/var/run/docker.sock"

    # Backup settings - fixed path inside container
    BACKUP_BASE_PATH: str = "/backups"

    # Default GFS retention policy (configurable per target in UI)
    DEFAULT_KEEP_LAST: int = 3
    DEFAULT_KEEP_DAILY: int = 7
    DEFAULT_KEEP_WEEKLY: int = 4
    DEFAULT_KEEP_MONTHLY: int = 6
    DEFAULT_KEEP_YEARLY: int = 2

    # Komodo Integration
    KOMODO_API_URL: str = ""
    KOMODO_API_KEY: str = ""
    KOMODO_ENABLED: bool = False

    # Timezone
    TZ: str = "Europe/Berlin"

    class Config:
        env_file = ".env"
        case_sensitive = True


settings = Settings()
