# DockerVault Storage, Recovery, and Komodo Design

## Status

Approved product direction from Max Ebert on 2026-07-11. This document defines the implementation boundaries and acceptance contracts for follow-up issues and pull requests.

## Goals

1. Make local and remote storage maintenance safe and understandable.
2. Support recursive deletion of selected backup directories without closing the storage browser.
3. Expose useful progress for long-running operations and honest indeterminate states when progress cannot be measured.
4. Show aggregate directory sizes for backup folders without blocking ordinary directory listing.
5. Export and restore DockerVault configuration as a portable, password-protected recovery bundle.
6. Let a fresh DockerVault installation restore such a bundle during initial setup and immediately rediscover its managed backup archives.
7. Improve the existing Komodo integration with stack-aware status and lifecycle orchestration.

## Non-goals

- The recovery bundle does not contain backup archives or volume data.
- The first recovery version does not fetch a bundle directly from S3, SSH, WebDAV, FTP, or Rclone during bootstrap.
- DockerVault does not deploy itself into Komodo.
- DockerVault does not mutate Komodo stack definitions, repositories, variables, or secrets.
- Progress percentages must not be fabricated when the backend cannot calculate total work.
- This work does not redesign unrelated backup, retention, authentication, or notification flows.

## Current behavior and verified problems

- The local delete endpoint delegates a selected path to file deletion. Deleting `/backups/actual-stack-backup` therefore raises `Errno 21: Is a directory` and returns HTTP 500.
- `StorageBrowser` contains selection state and a `selectAll()` helper, but users do not get an obvious, persistent Select All control for the current listing.
- Deletion is represented by a blocking request and toast. There is no durable operation state, and the user can lose the browser context instead of seeing completion details.
- Listed directory entries do not expose aggregate recursive sizes.
- The setup flow only supports creating a new installation.
- Komodo support can stop and start individual containers and read stacks, but does not provide a complete stack lifecycle or stack-aware backup workflow.

## Architecture and delivery sequence

The work is split into four implementation pull requests followed by an independent acceptance gate. Every implementation branch starts at the then-current `origin/develop`, and every pull request targets `develop`.

1. Storage deletion correctness and directory sizes.
2. Storage selection, durable operation progress, and browser-state UX.
3. Encrypted system recovery export and setup import.
4. Komodo stack lifecycle and backup orchestration.

The pull requests are merged serially so each later phase starts from the previously reviewed behavior.

## 1. Storage deletion correctness

### Path safety

All local paths are resolved relative to the configured local backup root. The backend rejects:

- absolute user-supplied paths;
- `..` traversal;
- symlink escapes from the configured root;
- deletion of the storage root itself;
- missing paths unless the request explicitly supports idempotent cleanup.

The resolved target is classified with a non-following filesystem check. Regular files and symlinks are unlinked. Directories are recursively removed without following symlinks outside the target tree.

### API behavior

Single and bulk deletion use one typed deletion service. Expected user errors return a safe 4xx response. Unexpected failures return a safe 5xx response and preserve detailed diagnostics only in server logs.

Bulk deletion returns per-item outcomes. One failure does not hide successful deletions, and a successful item is never reported as failed.

### Directory sizes

Ordinary listing remains fast. File sizes are returned directly. Recursive directory size is loaded asynchronously and cached by storage identity, path, modification fingerprint, and a bounded lifetime.

The UI initially displays an indeterminate size marker for directories, then updates each row as size results arrive. Size traversal does not follow symlinks and is cancellation-aware. Permission errors are shown as unavailable rather than as zero bytes.

## 2. Selection, operation progress, and browser state

### Selection

Selection mode exposes visible controls:

- Select All for every entry in the current directory;
- Clear Selection;
- selected count;
- an indeterminate checkbox when only part of the listing is selected.

Selection applies only to the current directory and is cleared when navigating to another directory. Files and directories can both be selected.

### Durable operations

Long-running destructive and archival actions use a shared backend operation model with:

- operation ID;
- action type;
- state: queued, running, completed, failed, or cancelled;
- total item count when known;
- completed, succeeded, and failed counts;
- current item with sensitive path prefixes removed where appropriate;
- optional bytes completed and total bytes when measurable;
- timestamps;
- bounded per-item error summaries.

The initial mutation returns an operation ID. The frontend receives updates through the existing realtime transport when available and uses bounded polling as a fallback. Reloading the page can reconnect to active operations.

### Honest progress presentation

- Multi-item deletion displays completed items divided by total items.
- A recursive directory deletion displays an indeterminate progress bar unless a safe pre-scan supplied a reliable total.
- Downloads, restores, encryption, synchronization, and backup stages use byte or step progress only where the backend has authoritative measurements.
- Operations with no reliable denominator use a spinner or indeterminate bar with the current stage.

### Storage browser lifecycle

Starting or completing an operation does not close the storage browser. The browser preserves storage target, path, selection mode, and scroll context. On completion it refreshes the affected listing and displays an in-panel summary containing succeeded, failed, and skipped items. The user closes the browser explicitly.

Destructive controls are disabled only for conflicting items, not for the entire storage browser. Failed items can be retried without reselecting successful items.

## 3. Encrypted DockerVault recovery bundle

### Bundle contents

The portable `.dockervault` bundle includes:

- a schema and application compatibility version;
- database records required for backup targets, schedules, retention policies, backup catalog, storage synchronization metadata, application settings, and Komodo settings;
- encrypted remote-storage and Komodo credentials;
- references and public metadata for managed SSH keys;
- an integrity manifest and export timestamp.

The bundle excludes:

- backup archives and volume contents;
- runtime logs not needed for recovery;
- sessions and transient operation state;
- plaintext passwords, API keys, secrets, private keys, or raw environment dumps.

Private SSH key material is excluded in the first version. Imported SSH-backed targets that require a private key are marked `credentials_required` until the operator supplies or mounts the key again. This avoids embedding filesystem-bound private keys in an otherwise portable settings bundle.

### Cryptography

The complete payload is authenticated and encrypted with a random data-encryption key. The export password derives a wrapping key using Argon2id with a random salt and versioned parameters. An established authenticated-encryption primitive from the project dependency stack, such as AES-256-GCM, encrypts the data key and payload.

Only a minimal non-secret format header, cryptographic parameters, salt, and nonce remain outside the encrypted payload. Wrong passwords, truncation, modification, unsupported versions, and malformed manifests fail closed before any live state changes.

Passwords are never logged, persisted, placed in URLs, or returned by APIs. Export responses disable caching. Temporary plaintext files are avoided; unavoidable temporary material uses restrictive permissions and deterministic cleanup.

### Export UX

An authenticated administrator opens System Backup in Settings, enters and confirms a dedicated export password, and downloads one `.dockervault` file. The UI explains that the file does not contain backup archives and that the password cannot be recovered.

### Setup recovery UX

When setup is required, the first screen offers:

1. Create a new installation.
2. Restore a DockerVault system backup.

Restore accepts the bundle and password, then runs these stages:

1. upload;
2. decrypt and authenticate;
3. validate schema and application compatibility;
4. validate relational references and storage paths;
5. show a redacted import summary;
6. import into an isolated temporary database;
7. atomically activate imported state;
8. create the new local administrator session;
9. test storage and Komodo connections;
10. reconcile the backup catalog against reachable archives.

No partial import reaches the active database. A failure before activation leaves setup mode unchanged. A failure after activation triggers rollback to the pre-import state.

### Backup reconciliation

Imported catalog entries remain authoritative historical records but gain a reachability state. DockerVault checks expected local and remote archive locations without deleting or rewriting them. Missing archives are marked unavailable. Discoverable archives that have valid DockerVault metadata can be proposed for catalog adoption through an explicit follow-up action.

## 4. Komodo integration

### Supported capabilities

DockerVault adds typed Komodo operations for:

- connection and version validation;
- server, stack, and container discovery;
- stack status and container status;
- stack stop, start, and restart;
- waiting for a requested stable state with timeout and progress;
- associating a backup target with a Komodo stack identity and server identity.

### Backup orchestration

For a Komodo-backed stack target, DockerVault can:

1. verify Komodo availability and resolve the configured stack;
2. capture the pre-backup stack state;
3. request a stack stop when required;
4. wait until relevant containers are stopped;
5. run the existing backup pipeline;
6. restore only the lifecycle state DockerVault changed;
7. wait for the stack to become healthy or report a bounded failure.

If Komodo is configured as the lifecycle authority, DockerVault does not silently fall back to direct Docker mutations. A failed or ambiguous stop blocks the backup before archive creation. Cleanup and restart behavior remains fail-closed and records every lifecycle action.

### UI

Target configuration displays discovered Komodo stacks instead of requiring free-form names where the API supports discovery. It shows server, stack status, last refresh, and connection errors. Manual Start, Stop, and Restart require confirmation and use the shared durable operation progress model.

### Capability negotiation

Komodo API support varies by version. DockerVault reads the Komodo version and exposes only verified capabilities. Unsupported operations are disabled with a clear explanation rather than attempted with guessed request names.

## Error handling and observability

- Every long-running action has a correlation-safe operation ID.
- User-facing messages redact credentials and sensitive absolute paths.
- Server logs retain actionable exception context without credentials.
- Recovery import writes a structured audit result without recording the bundle password or decrypted secrets.
- Komodo failures identify stage, target, and timeout category.
- Storage bulk actions preserve per-item failure information.

## Testing strategy

### Backend

- file, symlink, directory, traversal, root-delete, permission, and partial bulk-delete tests;
- directory-size calculation, symlink avoidance, cancellation, invalidation, and unavailable-size tests;
- operation state-transition, reconnect, progress, cancellation, and bounded-error tests;
- recovery round trip for every included model and encrypted credential type;
- wrong password, tampering, truncation, incompatible version, invalid reference, interrupted import, and rollback tests;
- explicit scans proving plaintext secrets never appear in bundle bytes, logs, API errors, or temporary files;
- Komodo contract fixtures for supported and unsupported versions, discovery, lifecycle success, timeout, partial failure, and state restoration.

### Frontend

- visible Select All and partial-selection behavior;
- browser remains open after success and failure;
- progress, indeterminate, reconnect, completion, retry, and mixed-result states;
- asynchronous folder-size loading and unavailable states;
- setup choice, bundle validation, redacted summary, import progress, rollback error, and successful handoff;
- Komodo discovery, capability-disabled controls, lifecycle confirmation, and progress.

### Integration and packaging

- clean database export/import round trip;
- existing populated database migration and import;
- containerized local directory deletion using a nested backup fixture;
- live-compatible mocked Komodo JSON-RPC requests at the HTTP boundary;
- full backend and frontend CI-equivalent checks;
- Docker image build and setup-wizard smoke test;
- secret scan, `git diff --check`, and repository-wide forbidden U+2013/U+2014 scan.

## Acceptance criteria

1. Deleting `actual-stack-backup` as a local directory succeeds recursively or returns a safe itemized error, never `Errno 21`.
2. The storage browser offers visible Select All and remains open after deletion.
3. Bulk deletion shows real completed and total counts and a final per-item summary.
4. Recursive operations with unknown totals use an indeterminate indicator instead of a fabricated percentage.
5. Backup-directory sizes load asynchronously without blocking the initial file listing.
6. An administrator can download a password-protected system bundle and restore it from the first-run setup flow.
7. Wrong passwords or modified bundles change no active state.
8. Imported targets, schedules, policies, catalog records, encrypted credentials, and Komodo settings are restored, while unavailable key files are clearly flagged.
9. A restored installation can reconcile and manage reachable existing backups without recreating all targets manually.
10. Komodo-backed targets can discover and safely stop, start, or restart stacks with status waits and fail-closed backup orchestration.
11. Every implementation PR is independently reviewed and merged into `develop` only after local and remote checks pass.
