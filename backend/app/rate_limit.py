"""Rate limiter instance shared across the application."""

from starlette.requests import Request

from slowapi import Limiter


def _get_real_client_ip(request: Request) -> str:
    """Extract real client IP from proxy headers, falling back to peer IP.

    Nginx sets X-Real-IP from the outermost client address.
    """
    return (
        request.headers.get("X-Real-IP")
        or request.headers.get("X-Forwarded-For", "").split(",")[0].strip()
        or request.client.host
        if request.client
        else "127.0.0.1"
    )


limiter = Limiter(
    key_func=_get_real_client_ip,
    default_limits=["120/minute"],
)
