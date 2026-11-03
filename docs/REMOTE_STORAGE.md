# Remote storage compatibility

## WebDAV object paths

Keep the configured WebDAV endpoint URL in URL form (including any existing
percent-encoded endpoint path). `base_path` and logical remote paths are filesystem
segments, not already-encoded URLs: literal `%`, `#`, `?`, spaces and Unicode are
encoded once when building requests. Collection creation, size verification,
listing, downloads and deletes use the same representation. No catalog/schema
change is required.

Older versions could address a truncated path when a name contained a URL
fragment/query delimiter. Existing remote objects are **not automatically renamed**
or trusted as the intended archive. Before relying on an old remote-only record,
inventory the provider, download and verify the actual archive/payload against a
known local copy or checksum, and move/copy it to the intended logical path only
when its identity is unambiguous. Re-upload from a verified local copy otherwise.
A collision may already have overwritten the older bytes; metadata/renaming cannot
recover them. Do not delete the last verified local copy based on a previously
acknowledged upload alone.

## FTP transport

`ftp` means plain, **unencrypted FTP**, including credentials and backup contents.
FTPS (explicit or implicit TLS) is not supported. Existing FTP configurations
retain their protocol and port; this release does not silently switch transport.
Use SSH/SFTP for a secure connection, and migrate intentionally by configuring and
testing that destination before removing any verified copy. Do not expose plain
FTP on an untrusted network.

Connections use aioftp's supported 30-second connection and 60-second socket/read
timeouts. Each operation opens and closes its own client, including failed login,
handshake, cancellation and data-operation failures; repeated calls are safe.
Uploads return the same `(success, error_message)` tuple as other providers.

## Verified synchronization

The manual sync endpoint uses the automatic sync orchestration, including key
sidecars and persistent per-storage outcomes. Only completed local archives and
enabled destinations are accepted. A failed transfer returns an HTTP error, never
a success message. Repeated requests update the same catalog record; a catalog
path owned by another backup is rejected before transfer. The manual action never
triggers the plan's automatic local-copy deletion option.

The shared engine claims `(storage ID, concrete object path)` for both the
archive and its sidecar in a global acquisition order, through transfer,
verification and catalog commit. Distinct target IDs/names that normalize to the
same destination therefore cannot both overwrite it and report success. Ownership
is re-read after waiting; an existing foreign archive **or sidecar**, including a
failed/partial catalog copy, rejects the operation with409. Cancelling a waiter
releases any already-acquired keys before transfer starts. Once transfer starts,
a child operation receives cancellation once and is joined before claims are
released, including repeated cancellation. Abort-capable transfers receive the
signal (S3 multipart and FTP ABOR); non-abortable copies, single PUTs and other
provider writes settle at their I/O boundary first. Verification downloads and
hash workers likewise finish before their temporary files are removed. Cancellation
still propagates, rather than acknowledging an interrupted transfer as completed;
cancelling an executor future alone cannot stop its writer.
Independent storage/path identities remain
independent. This uses the existing single application engine, not a distributed
provider-side lock or external coordination service.

Only the terminal `.enc` suffix maps to `.key`: `vault.enc/archive.enc.tar.enc`
uses `vault.enc/archive.enc.tar.key`. Upload, download and restore use this same
mapping even if the local sidecar has another basename; directory/stem substrings
are never rewritten. Existing ambiguous remote copies are not renamed or adopted
automatically; re-sync a verified local archive/key pair before deleting it.

All synchronization uploads are downloaded to temporary disk and compared with
the local artifact using SHA-256 before completion is recorded. This deliberately
costs an extra download and temporary disk space up to the largest artifact; a
provider success response alone is not evidence of a recoverable copy. The same
verification applies to the encryption key sidecar. Hashing runs outside the event
loop, reads bounded chunks, and temporary files are removed on failure/cancellation.
Previous untracked copies are not trusted merely because an old version returned
200: sync again from a verified local source to verify and catalog the copy.

## Retry eligibility

Retry uses the same verified archive/sidecar path and persistent enabled-storage
state, even with a warm client cache. Concurrent requests serialize on the target
lock and re-read outcomes; successful copies are not retried or duplicated.
A failed backup is accepted only for a recorded `Remote sync failed:` or
`Remote storage sync failed:` error, an existing failed sync record, and a local
archive matching its persisted size and SHA-256. Missing checksums, archive/job
failures, cancellations, or changed bytes are rejected rather than guessed safe.
Encrypted backups also require their sidecar. After every required copy succeeds,
the prior error is retained in `backup_metadata.recovered_remote_sync_error` and
status becomes completed; unsuccessful recovery preserves the backup error and
local artifacts. This does not regenerate the backup or delete its local copy.

## S3 multipart cancellation

After an UploadId is known, cancellation sends a bounded AbortMultipartUpload and
then re-raises cancellation, including repeated cancellation during cleanup.
A response arriving during cancelled creation is settled and its UploadId aborted.
Creation settlement and abort each have a 10-second budget. If the provider never
returns an ID or rejects/hangs abort, cleanup is logged as incomplete rather than
reported successful; configure a provider-side incomplete-multipart lifecycle
policy as defense in depth. The original upload error is preserved if abort fails.
SDK clients, including replaced/removed cached S3 clients, close during shutdown.

## S3 inventory

Listing uses the SDK's async ListObjectsV2 paginator and follows every continuation
until the inventory is complete, including empty final pages. Provider errors or
truncated responses without a continuation token fail instead of returning an
empty/partial success; the files API reports502 without exposing provider details.
The existing `{files, path}` response and `name`, `size`, `is_dir`, `modified` item
fields remain unchanged. Names retain the full key relative to the requested
folder prefix, so `a/same.tar` and `b/same.tar` remain distinct. Folder prefixes end
in `/` to avoid accidentally matching sibling names such as `reports-old`.

This preserves recursive object inventory, not a new delimiter-based directory
browser. There is no silent MaxItems cap. The existing list API materializes all
matching objects and therefore uses memory proportional to inventory size; a
future public paginated API would need an explicit continuation/truncation contract
rather than silently omitting backups. No schema/archive or storage-browser UI
redesign is included.
