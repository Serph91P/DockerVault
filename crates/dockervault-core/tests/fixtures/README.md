# Public synthetic compatibility fixtures

These are byte-for-byte copies of the original frozen Python control corpus for
revision `18d8c3d`. All identities, DEKs, passwords and credentials here are
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
