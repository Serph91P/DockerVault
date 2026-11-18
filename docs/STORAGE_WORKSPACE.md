# Storage workspace and explicit draft tests

`/storage` starts with System-Local (`storage=local`) and configured locations
(positive numeric IDs, including configured Local/NFS). They are different
identities even when they use the same provider or directory.

A location keeps its context in the query string:

- `storage=local|ID`, `tab=files|connection|usage` (omitted tab means files)
- `path`: relative directory, validated again by the backend
- `q`: filter of the current loaded directory, never a recursive provider search
- `kind=all|file|directory|link`, `sort=name|size|modified`, `direction=asc|desc`
- `operation`: the authorized backend operation ID, not a reconstructed request
- `returnTo`: a permitted local History/archive/list context only

Selection is not persisted in the URL or browser storage. Changing location,
root or session revokes it; changing folder/filter clears it with a notice.
Sort and confirmation cancellation preserve selection. Select All selects only
visible entries. Confirmed job admission clears selection, not the current path.
Existing #141/#142 directory and operation APIs remain authoritative; admission
is not deletion, cancellation is not rollback, and unknown is not retryable failure.

## Authenticated synchronous API: `POST /api/v1/storage/test-config`

Create draft:

```json
{"mode":"create","config":{"name":"Example","storage_type":"local","base_path":"/mounted/example","enabled":true}}
```

Edit draft:

```json
{"mode":"edit","storage_id":7,"config":{"base_path":"/mounted/other","password":""}}
```

`config` uses the existing StorageCreate or StorageUpdate fields. Provider type
is immutable in Edit (an attempted `storage_type` field is rejected). Omitted or
blank password/access-key/secret-key means keep; explicit `null` means clear,
subject to validation of the effective configuration. SSH key references must
remain within configured allowed directories. The effective configuration is a
private copy, never an ORM mutation or a temporary stored location.

The response is the existing `{ "success": boolean, "message": string }` shape.
Messages are static, credential-free observations. A private short-lived manager
is closed on success, error and cancellation; the shared manager is not replaced.
There is no database insert/update/commit or implicit enable operation.

Testing is optional and explicit. **Not saving is not read-only:** a Local test
may create its directory and creates/removes an exclusive, uniquely named write
probe. It must not overwrite `.backup_test` or any existing collision/symlink.
Cleanup checks the bound directory and open probe identities and fails closed
when either has changed. Cleanup failure is not success.

The UI binds draft results to the current private form revision and session.
An edit makes the observation stale; a lost response is unknown. Next, Back and
Test never create a location. Only the final reviewed Create sends a create POST;
an unconfirmed response must be inspected before another deliberate create.

## Capabilities, downloads and keys

Enabled is only a configuration flag. Test time is a session observation, not
continuous health monitoring. System-Local Usage consumes `disk_status` from
`/settings/system-info`: it measures the backup directory's whole filesystem,
not exclusive DockerVault archive usage. Unavailable fallback numbers are not
measurements. Configured Local/remote quota remains explicitly unavailable.
Asynchronous directory size is separate and uses the existing Local capability.

Single-file and selected ZIP downloads use existing resource-owning routes.
A requested browser download does not confirm that the operator saved it.
Single-file responses are browser-managed rather than fully buffered in JS;
visible error handling remains in the workspace. Removing a location configuration
is separate from deleting entries, and referenced configurations still return409.

SSH key administration lives once in Settings / Security at `#ssh-keys`.
Connection links there and returns to its workspace without preserving private
credential inputs. Generation, public copy/download, references, deletion and
password-assisted installation retain their existing contracts; FTPS, uploads,
paste and new remote scanners/providers are not introduced.
