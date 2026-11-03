"""Durable Komodo configuration: explicit DB field > environment > defaults."""

import asyncio
from dataclasses import dataclass, field
from ipaddress import ip_address
import socket
from urllib.parse import urlparse

from sqlalchemy import select, text

from app import database
from app.config import settings
from app.credential_encryption import decrypt_value, encrypt_value
from app.database import AppSettings


@dataclass(frozen=True)
class KomodoConfig:
    enabled: bool
    api_url: str
    api_key: str = field(repr=False)
    api_secret: str = field(repr=False)


def validate_komodo_url(url: str) -> str:
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("Komodo URL must include an http(s) scheme and hostname")
    if parsed.username or parsed.password or parsed.fragment or parsed.query:
        raise ValueError("Komodo URL must not contain credentials, query or fragment")
    try:
        parsed.port
    except ValueError:
        raise ValueError("Komodo URL has an invalid port") from None
    host = parsed.hostname.lower()
    if host in {"metadata.google.internal", "metadata.aws.internal", "169.254.169.254"}:
        raise ValueError("Metadata endpoints are not allowed")
    trusted = {
        h.strip().lower() for h in settings.KOMODO_ALLOWED_HOSTS.split(",") if h.strip()
    }
    env_host = urlparse(settings.KOMODO_API_URL).hostname
    if env_host:
        trusted.add(env_host.lower())
    try:
        addresses = socket.getaddrinfo(
            host, parsed.port or (443 if parsed.scheme == "https" else 80)
        )
    except socket.gaierror:
        raise ValueError("Cannot resolve Komodo hostname") from None
    resolved = [ip_address(address[4][0]) for address in addresses]
    if any(
        address.is_link_local or str(address) == "fd00:ec2::254" for address in resolved
    ):
        raise ValueError("Link-local/metadata endpoints are not allowed")
    if host not in trusted and any(not address.is_global for address in resolved):
        raise ValueError(
            "Private/internal Komodo endpoint requires an explicit operator-approved hostname"
        )
    return url.rstrip("/")


def effective_config(values: dict) -> KomodoConfig:
    def value(name: str) -> str:
        key = f"komodo_{name}"
        if key not in values:
            return getattr(settings, f"KOMODO_{name.upper()}")
        stored = values[key] or ""

        return decrypt_value(stored) if name in ("api_key", "api_secret") else stored

    return KomodoConfig(
        values["komodo_enabled"] == "true"
        if "komodo_enabled" in values
        else settings.KOMODO_ENABLED,
        value("api_url").rstrip("/"),
        value("api_key"),
        value("api_secret"),
    )


async def load_komodo_config() -> KomodoConfig:
    async with database.async_session() as session:
        rows = (
            await session.execute(
                select(AppSettings).where(AppSettings.key.like("komodo_%"))
            )
        ).scalars()
        return effective_config({row.key: row.value for row in rows})


async def save_komodo_config(changes: dict) -> None:
    # A SQLite write reservation serializes read/merge/validate/commit across processes.
    async with database.async_session() as session:
        await session.execute(text("BEGIN IMMEDIATE"))
        rows = {
            row.key: row
            for row in (
                await session.execute(
                    select(AppSettings).where(AppSettings.key.like("komodo_%"))
                )
            ).scalars()
        }
        values = {key: row.value for key, row in rows.items()}
        updates = {}
        for name, value in changes.items():
            if value is None:
                continue  # Legacy nullable input means omission; empty string explicitly clears.
            key = f"komodo_{name}"
            updates[key] = (
                ("true" if value else "false")
                if name == "enabled"
                else (
                    encrypt_value(value) if name in ("api_key", "api_secret") else value
                )
            )
        config = effective_config({**values, **updates})
        if config.api_url:
            await asyncio.to_thread(validate_komodo_url, config.api_url)
        if config.enabled and not all(
            (config.api_url, config.api_key, config.api_secret)
        ):
            raise ValueError("Enabled Komodo requires API URL, key and secret")
        for key, value in updates.items():
            if key in rows:
                rows[key].value = value
            else:
                session.add(AppSettings(key=key, value=value))
        await session.commit()
