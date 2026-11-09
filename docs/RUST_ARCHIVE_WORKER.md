# Native privileged archive worker (candidate)

`dockervault-archive-worker` is the fixed archive-creation target, not an
interpreter, shell, restore service, or recovery-bundle feature. Native service
admission/integration and the complete R6 acceptance remain unfinished.

## Boundary

- Install the executable root-owned and not writable by the service UID.
- The sudo policy permits only `/usr/local/libexec/dockervault-archive-worker`
  with the empty argument matcher `""`; the program independently rejects args.
- Read one version1 JSON request from stdin, bounded to1MiB. No credentials,
  database, scheduler, network or arbitrary executable is part of the protocol.
- Destinations must be beneath the fixed `/backups` directory capability.
  Existing files are never replaced. Parent components are opened no-follow.
- Approved real source roots are Docker volumes/overlay2 or the explicitly
  supplied host-path source prefixes from the caller's existing target snapshot.
  Opened source capabilities, not repeated pathname traversal, drive the walk.
- Directory symlinks are archived, not followed. Files use no-follow opens and
  opened-file inode/device checks. Hardlink identity records spill to private
  staging files rather than retaining every inode in a growing map.
- Tar output uses PAX where needed, including binary-name charset and fractional
  mtime. Compression uses gzip level6. Private staging is0700/0600 and output is
  synced before atomic no-replace publication. Cancellation is checked between
  bounded reads; the synchronous call returns only after file work settles.
- Output is handed back to the sudo caller's UID/GID before publication.

The helper preserves the admitted caller as **real UID**, retaining effective
and saved root only inside this fixed executable. Linux `kill(2)` therefore lets
the unprivileged owner terminate its own helper, including hard cancellation.
Without that identity contract, a real sudo test reproduced `EPERM` when the
owner tried to kill a root child. It is not safe to treat killing only the sudo
launcher as physical worker settlement.

## Reproducible isolated privilege probe

```sh
docker build -f docker/archive-worker-test.Dockerfile -t archive-worker-test .
docker run --rm --network none --user 1000:1000 \
  --entrypoint /usr/local/bin/archive-worker-privileges archive-worker-test
```

The test image is deliberately not a release/application image. Its synthetic
UID999/0400 input is unreadable by UID1000, but is correctly archived through the
fixed sudo policy and returned as UID1000/0600. The native probe also rejects
extra args/arbitrary commands and holds a real stdin read before checking
unprivileged SIGKILL plus physical reap. That is a **held-stdin** test, not an
assertion that every stalled source-filesystem read has been exercised.

Remaining mandatory work includes complete privileged timeout=0/nonzero and
archive-I/O/process-loss cases, service stdin/result settlement, bounded skipped
report transport, restore prevalidation/spooling/FD-safe mutation, complete file
metadata/type policy, all source/layout roundtrips and the full P matrix.
No full R6, R13 or application cutover is established by this checkpoint.
