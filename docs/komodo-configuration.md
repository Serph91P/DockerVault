# Komodo configuration authority

Every settings read/test, status query and existing Komodo operation resolves the same effective configuration:

1. An explicitly persisted `AppSettings` field overrides its environment counterpart.
2. An absent DB field falls back to `KOMODO_ENABLED`, `KOMODO_API_URL`, `KOMODO_API_KEY` or `KOMODO_API_SECRET`.
3. Environment defaults apply only when neither is configured.

The precedence is per field, preserving environment-only and mixed deployments. A persisted `false` or empty credential does not silently fall back to an old environment value. Legacy plaintext DB credentials remain readable; newly saved credentials use existing encryption at rest and never appear in API responses. Empty credentials may be saved while disabled; enabling requires the complete effective URL/key/secret.

Updates reserve one SQLite write transaction, validate the effective merged record and commit all changed fields together. Invalid input/commit failure leaves the previous record and client intact. The shared client refreshes after commit and before operations; independent processes read the same durable state. Request-specific headers use an immutable configuration snapshot, so rotating configuration does not close an in-flight session or send old default headers on later requests. Disabling prevents subsequent API lifecycle actions. This does not add backup/stack lifecycle features from #144.

## Intentional internal endpoints

An existing environment `KOMODO_API_URL` explicitly authorizes its hostname for the Komodo integration. For DB-only internal endpoints, the operator must configure exact comma-separated hostnames in `KOMODO_ALLOWED_HOSTS`. There is no global SSRF-disable switch, wildcard or substring match. Other private/loopback/link-local endpoints remain blocked, metadata endpoints remain blocked even when listed, and URL credentials/query/fragment are rejected. Redirects are not followed with credentials. Only operator-managed deployment configuration grants this exception; the Settings API cannot grant it.

No production environment, endpoint or access right is changed by this fix.
