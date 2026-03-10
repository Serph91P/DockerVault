"""
Komodo integration for external orchestration.

Komodo uses a JSON-RPC-style API where all requests are ``POST`` to
one of ``/read``, ``/write``, or ``/execute`` with a body of the form
``{"type": "<RequestType>", "params": {...}}``.

Authentication is via ``X-Api-Key`` / ``X-Api-Secret`` headers.

Reference: https://docs.rs/komodo_client
"""

import logging
from typing import Any, Dict, List, Optional

import aiohttp

from app.config import settings

logger = logging.getLogger(__name__)


class KomodoClient:
    """Client for the Komodo JSON-RPC-style API."""

    def __init__(self) -> None:
        self.api_url: str = settings.KOMODO_API_URL.rstrip("/")
        self.api_key: str = settings.KOMODO_API_KEY
        self.api_secret: str = settings.KOMODO_API_SECRET
        self.enabled: bool = settings.KOMODO_ENABLED
        self._session: Optional[aiohttp.ClientSession] = None

    # ------------------------------------------------------------------
    # Session management
    # ------------------------------------------------------------------

    @property
    def session(self) -> aiohttp.ClientSession:
        """Return (or lazily create) an ``aiohttp.ClientSession``."""
        if self._session is None or self._session.closed:
            headers: Dict[str, str] = {}
            if self.api_key:
                headers["X-Api-Key"] = self.api_key
            if self.api_secret:
                headers["X-Api-Secret"] = self.api_secret
            self._session = aiohttp.ClientSession(headers=headers)
        return self._session

    async def close(self) -> None:
        """Close the underlying HTTP session."""
        if self._session and not self._session.closed:
            await self._session.close()

    # ------------------------------------------------------------------
    # Low-level helpers
    # ------------------------------------------------------------------

    async def _post(
        self,
        path: str,
        request_type: str,
        params: Optional[Dict[str, Any]] = None,
        *,
        timeout: int = 10,
    ) -> Dict[str, Any]:
        """Send a JSON-RPC-style request and return the parsed response.

        Raises ``aiohttp.ClientError`` or ``ValueError`` on failure.
        """
        url = f"{self.api_url}{path}"
        body: Dict[str, Any] = {"type": request_type}
        if params:
            body["params"] = params

        async with self.session.post(
            url,
            json=body,
            timeout=aiohttp.ClientTimeout(total=timeout),
        ) as resp:
            resp.raise_for_status()
            return await resp.json()

    # ------------------------------------------------------------------
    # Health / availability
    # ------------------------------------------------------------------

    async def is_available(self) -> bool:
        """Check whether Komodo is reachable by fetching its version."""
        if not self.enabled or not self.api_url:
            return False

        try:
            data = await self._post("/read", "GetVersion", timeout=5)
            version = data.get("version", data)
            logger.debug("Komodo version: %s", version)
            return True
        except Exception as e:
            logger.warning("Komodo health check failed: %s", e)
            return False

    # ------------------------------------------------------------------
    # Container lifecycle
    # ------------------------------------------------------------------

    async def request_container_stop(
        self,
        container_name: str,
        server: str = "",
        *,
        reason: str = "backup",
    ) -> bool:
        """Request Komodo to stop a container via ``/execute``."""
        if not self.enabled:
            return True

        try:
            params: Dict[str, Any] = {
                "container": container_name,
            }
            if server:
                params["server"] = server

            await self._post("/execute", "StopContainer", params, timeout=30)
            logger.info(
                "Komodo stopped container %s (reason: %s)", container_name, reason
            )
            return True
        except Exception as e:
            logger.error("Komodo StopContainer failed for %s: %s", container_name, e)
            return False

    async def request_container_start(
        self,
        container_name: str,
        server: str = "",
        *,
        reason: str = "backup_complete",
    ) -> bool:
        """Request Komodo to start a container via ``/execute``."""
        if not self.enabled:
            return True

        try:
            params: Dict[str, Any] = {
                "container": container_name,
            }
            if server:
                params["server"] = server

            await self._post("/execute", "StartContainer", params, timeout=30)
            logger.info(
                "Komodo started container %s (reason: %s)", container_name, reason
            )
            return True
        except Exception as e:
            logger.error("Komodo StartContainer failed for %s: %s", container_name, e)
            return False

    async def get_container_status(self, container_name: str) -> Optional[Dict]:
        """Fetch container details from Komodo via ``/read``."""
        if not self.enabled:
            return None

        try:
            data = await self._post(
                "/read",
                "ListContainers",
                {"name": container_name},
            )
            # The response contains a list – find the matching container.
            containers: List[Dict] = data if isinstance(data, list) else []
            for c in containers:
                if c.get("name") == container_name:
                    return c
            return data if not isinstance(data, list) else None
        except Exception as e:
            logger.error("Komodo ListContainers failed for %s: %s", container_name, e)
            return None

    # ------------------------------------------------------------------
    # Stack operations (Komodo-native concept)
    # ------------------------------------------------------------------

    async def list_stacks(self) -> List[Dict]:
        """List Komodo stacks via ``/read``."""
        if not self.enabled:
            return []

        try:
            data = await self._post("/read", "ListStacks")
            return data if isinstance(data, list) else []
        except Exception as e:
            logger.error("Komodo ListStacks failed: %s", e)
            return []

    async def get_stack(self, stack_name: str) -> Optional[Dict]:
        """Get a single Komodo stack by name."""
        if not self.enabled:
            return None

        try:
            data = await self._post("/read", "GetStack", {"stack": stack_name})
            return data
        except Exception as e:
            logger.error("Komodo GetStack failed for %s: %s", stack_name, e)
            return None


# Global Komodo client singleton
komodo_client = KomodoClient()
