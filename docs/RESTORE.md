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
Relative symlink targets are not rewritten. Absolute names, paths escaping the
destination and unsafe links are still checked before extraction, and Python's
`data` extraction filter remains enabled. A recorded single-source root that does
not match the archive is rejected instead of silently restoring to another tree.
