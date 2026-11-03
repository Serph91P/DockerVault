# Restore destinations and archive layout

For a **single-source path or volume backup**, the selected/default destination
is the root of the restored data. An archive member `source/file` restores to
`<destination>/file`, not `<destination>/source/file`. Default path restore uses
the configured source directory; volume restore uses the current Docker volume
mountpoint. A custom destination gets the same contents mapping and leaves the
original source untouched.

New backups record version 1 `archive_layout` metadata with a `single` or `multi`
kind and the original single-source root. Existing prefixed archives without a
marker are supported: only path/volume targets infer and remove a single common
source-directory prefix. Rootless archives are left rootless. Container/stack
archives keep their mount/source prefixes, including when only one mount was
archived; their existing custom-destination semantics are unchanged.

Mapping remaps archive-root-relative hardlink targets together with file names.
Relative symlink targets are not rewritten. Directory symlinks are archived once
without traversing them; existing include/exclude rules apply to the link entry.
Safe relative links (including a link to the restore root) round-trip, while an
external target remains forbidden on extraction. Old archives cannot recover
links that were omitted when they were created. Absolute names, paths escaping the
destination and unsafe links are still checked before extraction, and Python's
`data` extraction filter remains enabled. A recorded single-source root that does
not match the archive is rejected instead of silently restoring to another tree.

Backup and restore ownership uses the actual canonical data paths, not only a
configured target or volume name. Volume/container/stack mountpoints therefore
share the same path lock as a restore to that directory (including symlink aliases),
with all keys acquired in one global order. The archive reader receives the frozen
source/mount snapshot resolved against claimed immutable container IDs before any
stop. It never reselects a replacement by name after stopping the original; a
missing/disagreeing identity fails closed instead of reading unclaimed live data.
