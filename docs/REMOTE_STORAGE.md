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
