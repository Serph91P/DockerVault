# Bounded archive browsing

Archive listing (GET `/backups/{id}/files` and POST `/backups/{id}/files` with a private key) now runs the blocking `tarfile` scan in a worker thread under a bounded concurrency gate, so large archives no longer monopolize the event loop.

`limit` (default and maximum `MAX_ARCHIVE_LISTING` = 2000) and `offset` paginate results with `X-Total-Count`/`X-Has-More` response headers, so responses and memory stay bounded independently of archive size. Requests beyond the limit are rejected with 422. Omitting both parameters preserves the existing flat-list compatibility path (capped at the same ceiling). Pagination is deterministic over the archive member order, so pages have no duplicates or omissions.

Cancellation and error paths still remove owned decrypted/remote temp files via the existing `finally` cleanup. No storage architecture change; encrypted archives are not decrypted onto disk during plain listing.
