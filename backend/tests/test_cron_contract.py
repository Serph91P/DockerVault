"""Preview must describe actual native job fire instants, not another parser.

These parity cases do not choose a new numeric weekday/DOM-DOW policy: the
configured execution trigger is the source of truth. Separate contract tests
will pin the owner-selected syntax and existing-data treatment.
"""

from datetime import datetime, timezone

import pytest

from app.config import settings
from app.scheduler import BackupScheduler


@pytest.mark.parametrize("zone", ["UTC", "Europe/Berlin"])
@pytest.mark.parametrize(
    "expression,base_text",
    [
        ("0 3 * * 1-5", "2026-11-01T00:00:00+00:00"),
        ("0 3 13 * fri", "2026-11-01T00:00:00+00:00"),
        ("0 3 * * sun", "2026-11-01T00:00:00+00:00"),
        ("0 3 * * *", "2026-11-01T00:00:00+00:00"),
        ("30 2 * * *", "2026-03-28T00:00:00+00:00"),
        ("30 2 * * *", "2026-10-24T00:00:00+00:00"),
    ],
)
def test_preview_tracks_configured_native_fire_instants(
    monkeypatch, zone, expression, base_text
):
    monkeypatch.setattr(settings, "TZ", zone)
    scheduler = BackupScheduler()
    trigger = scheduler._trigger(expression)
    assert str(trigger.timezone) == zone
    base = datetime.fromisoformat(base_text)
    for _ in range(6):
        # A preview is the next run strictly after its supplied base instant.
        actual = trigger.get_next_fire_time(base, base)
        assert actual is not None
        preview = scheduler.get_next_run(expression, base)
        assert preview.astimezone(timezone.utc) == actual.astimezone(timezone.utc)
        assert preview.timestamp() > base.timestamp()
        base = actual
