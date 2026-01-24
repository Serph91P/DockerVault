"""
Configuration settings for the backup manager.
"""

from pydantic_settings import BaseSettings
from typing import List
import os


class Settings(BaseSettings):
    """Application settings."""
    
    # Database
    DATABASE_URL: str = "sqlite+aiosqlite:///./data/backups.db"
    
    # Docker
    DOCKER_SOCKET: str = "/var/run/docker.sock"
    
    # Backup settings
    BACKUP_BASE_PATH: str = "/backups"
    DEFAULT_RETENTION_DAYS: int = 30
    DEFAULT_RETENTION_COUNT: int = 10
    MAX_CONCURRENT_BACKUPS: int = 2
    
    # Compression
    COMPRESSION_LEVEL: int = 6  # 1-9, higher = more compression
    
    # CORS
    CORS_ORIGINS: List[str] = ["http://localhost:5173", "http://localhost:3000"]
    
    # Komodo Integration
    KOMODO_API_URL: str = ""
    KOMODO_API_KEY: str = ""
    KOMODO_ENABLED: bool = False
    
    # Security
    SECRET_KEY: str = "change-me-in-production"
    
    # Scheduling
    SCHEDULER_TIMEZONE: str = "Europe/Berlin"
    
    class Config:
        env_file = ".env"
        case_sensitive = True


settings = Settings()
