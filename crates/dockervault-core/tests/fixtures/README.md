# Public synthetic compatibility fixtures

Original archive/sidecar fixtures are byte-for-byte copies of the frozen Python
control corpus for revision `18d8c3d`. All identities, DEKs, passwords and credentials here are
**PUBLIC SYNTHETIC TEST MATERIAL**, never operator or production keys.

The `*.key.TEST_ONLY` files contain **age-encrypted sidecar ciphertext**, not
private identities. The explicit test-only suffix distinguishes fixture storage
from live key-file paths for repository tooling. To map them to the original
frozen corpus, remove only the final `.TEST_ONLY` from `*.key.TEST_ONLY`.
The two public synthetic identities are stored as labelled numeric raw32 test
vectors in `age-identity-recipes.TEST_ONLY.json`. The shared test-only helper uses
the already pinned Bech32 standard codec to reconstruct the exact original
serialized bytes at test time; original SHA256 assertions and unchanged Python
ciphertexts enforce identity parity. CLI tests write only private0600 temporary
identity files, removed by their fixture owner. This is fixture representation,
not production key storage, rotation, a scanner exception or a changed oracle.
The immutable original R0 corpus is retained outside the maintained product.

This storage convention does not change the product's terminal-only `.enc` to
`.key` naming contract. Tests continue to assert that product naming separately;
the offline CLI accepts an explicit sidecar filename via `--key`.

No original frozen control, manifest, assertion or scanner configuration is
changed by these fixture copies.

The additional `pax-links.ascii.tar.gz*` ciphertexts use the same original gzip
input and public gzip recipient but a second synthetic test-vector DEK: 32 ASCII
`A` bytes. They were captured in the isolated Python `production` reference at
`c7a5594432c82d46c20e2d17068a1e85e8b23957`: its `encrypt_file`/`encrypt_dek` for
v1, and OpenSSL `enc -aes-256-cbc -pbkdf2 -iter 100000` with the hex DEK plus an
ordinary raw-DEK `age -a` sidecar for legacy. Native CI consumes only these
frozen expectations, not Python to regenerate them.

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
