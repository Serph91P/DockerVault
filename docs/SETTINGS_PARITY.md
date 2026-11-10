# Focused Settings parity (#190)

Backend responses are authoritative. A private draft, a completed HTTP write, a
canonical readback, a connectivity observation and lifecycle authority are separate
facts. Sections use durable anchors; browser Back/Forward/reload preserve context.
Only the selected context is shown. Independent section reads retain their own
last confirmed snapshots when another read fails.

| Existing capability | Location | Preserved API/contract |
|---|---|---|
| System version, uptime, counts, database size/path, backup directory, timezone | `#system` | GET `/settings/system-info`; read-only. Paths may be redacted by backend. |
| Disk capacity | `#system` | Same read; `disk_status=available` required for measured bytes/meter. Fallback zeros are not measurements. |
| Docker health and refresh | `#connection` | GET `/docker/health`; unknown/loading/error/stale distinct from connected. No invented configured socket value. |
| Komodo enablement, endpoint, key, secret | `#connection`, Edit Settings | GET/PUT `/settings/komodo`; DB > ENV > defaults. Redacted flags are not credentials. Empty masked inputs omit credentials (keep); explicit Clear sends empty string (DB override, not ENV reset). Endpoint unchanged is omitted; edited empty endpoint explicitly clears. Enablement is required by the existing PUT API. |
| Komodo Save / Cancel | `#connection` | Private memory only. PUT followed by validated canonical GET before success; failed readback is unconfirmed. Cancel drops private input, not an already accepted server write. Revision/session/unmount guards reject late results. |
| Komodo test | `#connection`, Test saved connection | POST `/settings/komodo/test` only; never saves a draft. Dirty/pending drafts must be saved or cancelled first. Dated UI-session observation bound to the read configuration, invalidated by edits/changed or stale server values. |
| Age setup/public key/copy/export/custody confirmation | `#encryption` | Existing single EncryptionSetup; GET `/encryption/status`, POST setup/confirm. One-time private key remains private to current custody dialog, export initiation is not custody confirmation (#181). |
| Age enable/disable, recovery instructions, regenerate warning/confirmation | `#encryption` | Existing encryption APIs and archive/key effects unchanged. Regeneration does not re-encrypt old archives; their old identity remains necessary. Not a login/bundle password change. |
| SSH list/generate/copy/public `.pub` download | `#ssh-keys` within Encryption & SSH keys | Existing single SSHKeyManager, GET/POST `/storage/ssh-keys` and public download route; real ed25519 filesystem keys. Private key never downloaded. |
| SSH delete/references | `#ssh-keys` | Existing confirmation and DELETE route; current storage references and consequence warning are shown. Backend does not prevent referenced-key deletion; the UI snapshot is not a concurrency guarantee. |
| Password-assisted SSH public-key install: auto / Hetzner / generic authorized_keys | `#ssh-keys` | Existing install route/methods; private in-memory password, no Mutation cache closure; fields frozen during install, password cleared on settlement. Separate completed/partial/accepted/failed/unknown observation, no nonfinal success. |
| Storage provider return | `#ssh-keys?storageReturn=…` (query precedes hash) | Existing allowlisted Storage workspace context preserved; one manager, no duplicate Storage key manager. |
| Pre-upload quota check | `#storage-safeguards` | GET/PUT `/settings/storage`; actual boolean required. Switch creates a private draft, Save reads back canonical boolean; Cancel restores latest confirmed value. Unknown is not false. No capacity reservation guarantee. |
| System recovery | `#system-recovery` | Authenticated administrator POST `/recovery/export` downloads a password-confirmed, no-store `.dockervault` bundle. First-run Create New versus Restore precedes account creation; POST preview validates offline, POST restore stages and atomically activates with a new local administrator/session. GET `/recovery/status` provides import-time connection/catalog observations and public SSH reconnect references, never private-key custody. Archives/volumes/private SSH/external age identities are excluded; passwords stay out of browser persistence and query/mutation cache. Existing archive restore links to History. |

## Deliberately not editable / not invented

Docker socket, backup path, concurrency, timeout and runtime configuration remain
deployment-controlled. The current settings API does not supply CPU/RAM metrics;
none are fabricated. GET `/komodo/status`, POST `/komodo/test`, container actions,
and discovery APIs remain their existing API contracts, not new Settings lifecycle
controls. #144 negotiated stack/lifecycle capabilities are absent: connectivity or
a version response never grants stop/start/restart authority. Account/password
management and other UI-unexposed API features are not silently added for parity.

## Navigation and accessibility

Native React Router data-router `useBlocker` guards section links, return links and
Back/Forward; Cancel/Escape returns to editing. Native `beforeunload` handles hard
reload/close. Existing App routes are preserved under the data-router root.
Named regions receive focus after direct anchors and history navigation. Fields
have labels/help/validation references; contextual save/test results are live
status regions. Private drafts are never put in URLs, persistent browser storage,
Query data/keys or Mutation data/variables/callback closures.

## Acceptance and executable evidence

| Original AC | Verification |
|---|---|
| 1. Every current editable setting and key/connection action has a documented new location and preserves its supported API behavior; system-owned/read-only values cannot be submitted as if editable. | This parity inventory, FocusedSettings/SettingsSSHPrivacy tests, native `run_settings_browser.py`; existing storage/overview return journeys. |
| 2. Save, Cancel and Test are separate. Failed tests do not silently save credentials; failed saves retain safe input and show field/section errors; reload returns canonical persisted values. | FocusedSettings causal RED/GREEN; native saved-only Komodo test, Save→GET→reload, SQLite effect checks and quota restart. |
| 3. Dirty navigation, simultaneous section loading, 422/401/503, environment-controlled settings and changed backend values have clear behavior without falsely showing defaults as saved. | Unit session/revision/readback/loading controls; native fault-injected HTTP responses clearly separated from real invalid-URL rejection, ENV/DB/client-refresh and actual process restart. |
| 4. Docker/Komodo mode and capabilities match real responses. Unsupported operations are explained rather than offered as functional controls. | Explicit #144 guidance, no lifecycle controls; actual settings API against loopback synthetic Komodo HTTP peer, not external Komodo authority. |
| 5. Age key workflows satisfy #181; SSH actions preserve existing functionality; no secrets appear in URLs, logs, local drafts or screenshots/test reports. | Existing EncryptionSafety suites plus native exported identity→real age archive decrypt; real SSH generation/copy/download/delete/reference files and explicitly synthetic remote transport; no screenshot while private material is visible. |
| 6. #143 integration has a distinct system-recovery flow and verified backend dependency, or remains absent with a clear dependency rather than a pretend working feature. | Real export, first-run validation/activation and canonical result flows now implemented. Native #143 acceptance is an additional gate, not replaced by the older dependency-only assertion. |
| 7. Keyboard, narrow viewports and screenreader checks cover section navigation, named controls, field help/errors and contextual save-result announcements. | Native keyboard/focus/Back/Forward, five CSS widths320/390/768/1024/1440, semantic controls/help/live regions and axe. Automated semantics are not actual human screenreader/physical-device acceptance; Max owns manual #191 checks. |

Run `python backend/tests/run_settings_browser.py IMAGE BROWSER_IMAGE AXE_JS OUT`
with Docker SDK available. It uses immutable standard-image assets/backend plus
explicit test-only fixtures, real cookie auth/SQLite/files, no production socket or
host bind. The eleven existing native runtime groups remain mandatory; this new
Settings group is additional. Raw dated results/image/source binding live in the
local handoff evidence, never fabricated in this inventory. No ARM64, external
provider, physical-device, native-AT, #191 or whole-#179 acceptance follows from
these isolated Linux checks.
