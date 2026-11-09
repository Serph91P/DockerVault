# Public synthetic restore fixture

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
