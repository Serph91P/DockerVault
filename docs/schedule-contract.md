# Schedule interpretation

DockerVault uses APScheduler 3's existing execution semantics, not Unix cron:

- Five fields: `minute hour day month weekday`.
- Monday is `0`, Sunday is `6`. Prefer named weekdays (`mon` through `sun`), e.g. `0 3 * * sun`.
- Day-of-month **and** weekday must both match. `0 3 13 * fri` means Friday the 13th, not every Friday or every 13th.
- Macros (`@daily`), extra seconds/year fields and expressions without a future fire time are rejected before saving.
- The server's `TZ` applies to validation, registration and preview. APScheduler's native wall-clock/DST behavior applies, including skipped or repeated local times. API preview timestamps include the offset; the wizard renders those instants in the browser's local timezone and identifies the server timezone.

`POST /api/v1/schedules/preview` accepts `cron_expression`, optional timezone-aware `base_time`, and `count` (1-10, default 3). Every returned instant comes from the same trigger factory used for jobs. Preview is strictly after the supplied base instant.

## Existing configurations

No cron strings are rewritten and valid jobs keep their effective weekday and day-of-month semantics. `GET /api/v1/schedules/compatibility/inventory` identifies numeric weekdays, combined day/weekday constraints and invalid stored expressions. Schedule/target responses also expose `cron_warnings`; the Schedules page displays the inventory, including legacy target schedules. Invalid stored jobs remain visible for correction rather than being silently converted. Numeric expressions may be rewritten by the user using names after reviewing intent; no production migration is performed.
