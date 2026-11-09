# Native offline archive recovery (candidate)

This is the native replacement target for the existing single-archive recovery
helper. It is **not** a system recovery bundle, setup/activation protocol, or the
separate full recovery-bundle feature. Service/API/frontend migration remains
pending the reviewed live-base integration; do not infer that the currently
published Python download endpoint has already changed.

## Recover an existing backup

```sh
dockervault-recover \
  --archive backup.tar.gz.enc \
  --output backup.tar.gz \
  --identity my-private-identity.txt \
  --key backup.tar.gz.key
```

The identity file accepts ordinary age X25519 identities, comments, and multiple
identities. Private identities are read locally, not placed in arguments, sent to
a server, cached in a database, or printed in error messages. `--key` names the
age-wrapped sidecar, not a raw key argument.

The executable requires the platform's compatible C runtime and OpenSSL 3
`libcrypto` library, but **does not invoke** Python, age, OpenSSL, Docker, a server,
or a database. Provision the binary and its runtime libraries before disconnecting
an offline machine. The candidate native image packages these libraries itself;
it is a test/build artifact, not a published release.

For the existing authenticated format, the complete DVENC-v1 ciphertext/header
MAC and CBC padding are checked before output publication. Decoding occurs only
in private staging (directory0700/file0600). Successful output is fsynced and
atomically renamed. Corruption, wrong keys and cancellation before publication
leave an existing output unchanged and clean up owned staging. If the final
post-rename directory sync fails, the explicit diagnostic reports publication
with uncertain durability rather than falsely claiming nothing changed.

Legacy raw32-byte age key envelopes also permit the historical unauthenticated
Salted__ CBC format. The CLI warns about this weaker legacy contract. Successful
padding, gzip CRC or tar readability is **not** authentication. A v1 key envelope
never permits a stripped-header downgrade to legacy CBC.

SIGINT/SIGTERM request cancellation; owned file/crypto work settles before exit.
A successfully committed publication wins a cancellation arriving afterwards.
The CLI returns0 on success,2 for invalid invocation,1 for recovery failure, or
128+signal when cancellation interrupts the operation.

## Build the standalone target

```sh
docker build -f docker/recover.Dockerfile -t dockervault-recover-candidate .
```

This native multi-stage build pins its Rust and Debian base-image indices and
uses the locked, security-qualified dependencies. It does not install a host Rust
toolchain. The runtime image runs as UID1000 and contains no Python, age/OpenSSL
CLI, application server or application database. Runtime distributions retain
the dependency/source notices described in `RUST_DEPENDENCY_NOTICES.md`.

Local evidence covers amd64 recovery of original Python v1 and legacy fixtures,
independently byte-verified, with network disabled and read-only root; the same
real standalone binary was also executed directly with empty environment/PATH.
Native arm64, the service custody/onboarding consumers, exported instruction
migration, full performance matrix and the final release packaging/security gates
remain required work. This document is not an R5/R13 or whole-rewrite acceptance.
