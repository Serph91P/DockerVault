# Native offline archive recovery (candidate)

This is the Rust standard image's single-archive recovery tool. It is **not** a
system recovery bundle, setup/activation protocol, or the separate full
recovery-bundle feature. The published Python releases still use their own
recovery script; a draft or local build does not change those releases.

## Save the native candidate's recovery tool

The native application built from this checkout serves `/recovery-tools/manifest.json` and
`/recovery-tools/dockervault-recover`. Save both from the same trusted candidate
before disconnecting. The manifest identifies the executable's SHA-256, source
revision and platform (`linux/amd64` or `linux/arm64`); the download matches the
**server image's** architecture, not necessarily the machine running the browser.
Check the platform and checksum before use. The checksum detects mismatched or
damaged files; it does not authenticate a manifest from an untrusted server.
This is not a replacement for release attestations or a download for macOS or
Windows. `/recover_backup.py` still belongs to the Python reference, not to
the candidate.

## Recover an existing backup

After verifying the manifest and checksum, make the downloaded file executable
(`chmod 700 dockervault-recover`). Then run:

```sh
./dockervault-recover \
  --archive backup.tar.gz.enc \
  --output backup.tar.gz \
  --identity my-private-identity.txt \
  --key backup.tar.gz.key
```

Extract the recovered archive only after exit status 0. Verify legacy CBC
archives against an independently trusted original checksum before extraction.

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

The standalone target has recovered original Python v1 and legacy fixtures on
actual native amd64 and arm64 hosted runners, with independent byte comparisons,
network disabled and read-only root. The same real standalone amd64 binary also
ran directly with empty environment/PATH. These are foundation checkpoints, not
a complete native application runtime acceptance. Service custody/onboarding,
exported instruction migration, full performance matrix and the remaining
application architecture/packaging/security gates remain required work. This
document is not an R5/R13 or whole-rewrite acceptance.
