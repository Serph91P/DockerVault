# Public synthetic service fixtures

`insert-state-source.TEST_ONLY.json` was captured from the unchanged accepted
#143 provider `fc1b3392fec6ecf9562ef88875ab143858d637bf` by actually invoking
`recovery_service.insert_state` on isolated in-memory SQLite. It contains the
synthetic input and raw SQL values for all nine portable tables, after decrypting
only synthetic storage/Komodo fields for comparison. Keys are generated only in
container RAM; no key or encrypted token is included. No app lifespan, live DB,
provider connection, archive/key file or production credential was accessed.
It checks SQL datetime/enum/boolean/opaque JSON binding, Python path relocation,
null/empty/plaintext credential handling, and non-secret settings. Native tests
add separate ciphertext-at-rest, caller-only commit/rollback, late SQL-fault,
physical SQLite update-hook cancellation, and extension-column preservation
assertions. This is logical row insertion evidence, not activation/cutover.


`global-pax.TEST_ONLY.tar` contains only four public bytes (`data`). It was
produced by unchanged Python 3.14.10 `tarfile`, PAX_FORMAT with global
`path=global/payload`, and a regular member whose base header says `base/payload`.
No application import, service startup, product data or secret was used.

SHA256: `b8e743ee86730ef000bed60d2f3f0a81fcbdc1043b43aa8be6f008a2f84112ab`.

Independent Python reading yields exactly `global/payload`, type regular, size4.
The qualified standard tar0.4.46 reader instead exposes XGlobalHeader and
`base/payload`; source inspection confirms global extensions are not applied.
The native bounded metadata reader uses the same library's Header and public
PaxExtensions primitives, with explicit bounded extension handling, rather than
trusting that default iterator or using unpack() on a live destination.

This fixture does not establish complete tar/GNU/PAX/data-filter parity.

`restore-semantics.TEST_ONLY.json` records nine additional real executions of the
unchanged original BackupEngine._extract_tar, compiled from its AST without app
imports/startup. Only synthetic old/new/data strings and logical fixture names
are included. It covers duplicate file/hardlink updates, contained aliases,
lexical parent normalization, forward hardlink failure and type conflicts.
Original error cases that leave partial target content are adverse controls:
the native all-member prevalidation requirement must reject before any live
write, not reproduce those partial effects. Complete native replay assertions
for these controls remain part of the unfinished destination planner.
