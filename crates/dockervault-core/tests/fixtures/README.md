# Public synthetic compatibility fixtures

Original archive/sidecar fixtures are byte-for-byte copies of the frozen Python
control corpus for revision `18d8c3d`. All identities, DEKs, passwords and credentials here are
**PUBLIC SYNTHETIC TEST MATERIAL**, never operator or production keys.

The four `*.key.TEST_ONLY` files contain **age-encrypted sidecar ciphertext**, not
private identities. The explicit test-only suffix distinguishes fixture storage
from live key-file paths for repository tooling. To map them to the original
frozen corpus, remove only the final `.TEST_ONLY` from `*.key.TEST_ONLY`.
The original `*.identity.TEST_ONLY` names and contents are unchanged.

This storage convention does not change the product's terminal-only `.enc` to
`.key` naming contract. Tests continue to assert that product naming separately;
the offline CLI accepts an explicit sidecar filename via `--key`.

No original frozen control, manifest, assertion or scanner configuration is
changed by these fixture copies.

`credential-framing.TEST_ONLY.json` contains 2,478 transformation recipes and
actual results from the unchanged accepted #143 provider `fc1b339` on Python
3.14.10/cryptography. Recipes mutate the existing public ciphertext or a key
DERIVED AT TEST TIME from public bytes0..31; no credential-shaped key is stored.
Both key construction and authenticated recovery are checked, including accepted
keys that intentionally cannot authenticate the original token. The source's
non-validating Base64 framing ignores non-alphabet bytes/interior padding but
requires sufficient trailing padding; use current CPython 3.14 behavior rather
than an older decoder's stop-at-padding rule. Native code only canonicalizes this
framing before the unchanged Fernet primitive; it never weakens authentication,
introduces key rotation, changes formats or changes strict bundle JSON/Base64.

`canonical-packages.TEST_ONLY.json` records SHA256 of actual unchanged accepted
`fc1b339` `package_rclone` output for all 87 accepted canonical import controls,
plus three focused empty/subset selections of the shared-file graph (90 total).
Inputs remain in the unchanged `rclone-packages.TEST_ONLY.json`; `indices` selects
storage rows in order. Hash bytes are Python `json.dumps(sort_keys=True,
separators=(',', ':'), ensure_ascii=True).encode()` of the complete output, not
native-generated expectations. Explicit source file records and provider options
prevent ambient/default credential or filesystem discovery in the control.
These are canonical managed-package controls, not proof of raw external config
normalization, provider transfers or service activation.

`rclone-environment.TEST_ONLY.json` records the actual accepted provider's
simple-uppercase/titlecase fallback: five explicit names and a SHA256 stream for
all 142,943 valid single-scalar names (big-endian u32 code point, UTF-8 environment
name for `known-hosts-file`, NUL delimiter). It preserves Unicode16 semantics
instead of adopting Rust's newer uppercase pairs, with no dependency/version
change. These metadata names contain no credentials.
