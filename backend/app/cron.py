"""One APScheduler 3 contract for stored jobs, validation and previews."""

from datetime import datetime, timezone
from zoneinfo import ZoneInfo

from apscheduler.triggers.cron import CronTrigger

from app.config import settings

CONTRACT = (
    "Five fields: minute hour day month weekday. Monday=0, Sunday=6; prefer mon..sun. "
    "Day-of-month AND weekday must both match. Times use the server timezone and "
    "native APScheduler wall-clock/DST behavior; macros are not supported."
)


def cron_trigger(expression: str) -> CronTrigger:
    return CronTrigger.from_crontab(expression, timezone=settings.TZ)


def next_run(expression: str, base: datetime | None = None) -> datetime:
    base = base or datetime.now(timezone.utc)
    if base.tzinfo is None:
        # Preserve the Python caller's legacy wall-clock input in the server TZ.
        base = base.replace(tzinfo=ZoneInfo(settings.TZ))
    result = cron_trigger(expression).get_next_fire_time(base, base)
    if result is None:
        raise ValueError("Cron expression has no future fire time")
    return result


def cron_warnings(expression: str | None) -> list[str]:
    if not expression:
        return []
    try:
        next_run(expression)
    except (ValueError, TypeError) as exc:
        return [
            f"Invalid stored schedule; cannot run until corrected: {exc}. {CONTRACT}"
        ]
    parts = expression.split()
    warnings = []
    if parts[4] != "*" and any(c.isdigit() for c in parts[4]):
        warnings.append(
            "Numeric weekdays retain existing execution: Monday=0, Sunday=6 (not Unix). "
            "Review intent; prefer mon..sun. No automatic conversion was performed."
        )
    if parts[2] != "*" and parts[4] != "*":
        warnings.append(
            "Day-of-month AND weekday must both match, not Unix OR. "
            "Existing execution is unchanged; review intent."
        )
    return warnings
