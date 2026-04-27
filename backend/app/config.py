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

    # Security
    CORS_ORIGINS: str = "http://localhost"
    COOKIE_SECURE: bool = True  # Safe default for production behind TLS proxy
    CREDENTIAL_ENCRYPTION_KEY: str = ""
    ALLOWED_HOOK_COMMANDS: str = (
        "pg_dump,pg_dumpall,mysqldump,mongodump,redis-cli,mariadb-dump"
    )
    HOOK_COMMAND_TIMEOUT: int = 300
    ALLOWED_SSH_KEY_DIRS: str = "/app/data/ssh_keys,/root/.ssh"
    ENABLE_DOCS: bool = False

    # Logging
    LOG_LEVEL: str = "INFO"
    LOG_FORMAT: str = "text"

    # Shutdown
    SHUTDOWN_TIMEOUT: int = 30

    # Backup engine — tar worker subprocess
    # 0 = no timeout (recommended for very large volumes); otherwise seconds.
    TAR_WORKER_TIMEOUT: int = 0
    # Idle timeout in seconds: kill worker only when no progress is reported.
    # 0 = disabled (relies on TAR_WORKER_TIMEOUT only).
    TAR_WORKER_IDLE_TIMEOUT: int = 1800

    # Remote storage — uploads
    # Number of upload attempts on transient errors (network, 5xx, etc.).
    STORAGE_MAX_RETRIES: int = 6
    # Verify remote checksum/size after upload (recommended).
    STORAGE_VERIFY_AFTER_UPLOAD: bool = True
    # Streaming chunk size for file uploads (1 MiB).
    STORAGE_UPLOAD_CHUNK_SIZE: int = 1024 * 1024
    # S3 multipart threshold (bytes). Files >= this size use multipart upload.
    S3_MULTIPART_THRESHOLD_BYTES: int = 64 * 1024 * 1024  # 64 MiB
    # S3 multipart chunk size (bytes).
    S3_MULTIPART_CHUNK_SIZE: int = 16 * 1024 * 1024  # 16 MiB

    # Komodo Integration
    KOMODO_API_URL: str = ""
    KOMODO_API_KEY: str = ""
    KOMODO_API_SECRET: str = ""
    KOMODO_ENABLED: bool = False

    # Timezone
    TZ: str = "Europe/Berlin"

    class Config:
        env_file = ".env"
        case_sensitive = True


settings = Settings()
