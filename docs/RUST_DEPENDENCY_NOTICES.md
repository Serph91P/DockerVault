# Rust dependency notices

The pinned Rust dependencies and licenses are recorded in `Cargo.lock` and checked
by `cargo deny`. Runtime distribution must retain the corresponding license texts,
notices and source-availability information alongside its SBOM.

## Mozilla Fernet 0.2.2

The unmodified `fernet` crate is licensed under **MPL-2.0**. DockerVault links the
published implementation rather than implementing a new Fernet cryptosystem.
The bounded package/version allowance in `deny.toml` does not waive advisories or
allow other copyleft dependencies.

- License: <https://www.mozilla.org/MPL/2.0/>
- Exact published source and license: <https://docs.rs/crate/fernet/0.2.2/source/>
- Source archive: <https://crates.io/api/v1/crates/fernet/0.2.2/download>
- Upstream: <https://github.com/mozilla-services/fernet-rs/>

MPL obligations apply to the covered dependency files; they are not a change to
DockerVault's own license. Final image/offline packaging must include the full
MPL license and this source-availability notice. Packaging verification remains
part of the Rust image/recovery deliverable, not established by this notice alone.

## CPython compatibility algorithms

Character-class range normalization in `dockervault-core/src/filters.rs` adapts
CPython's `fnmatch.py` behavior to native Rust tokens (no Python interpreter).
Public SSH metadata decoding in `dockervault-core/src/ssh_public.rs` also adapts
CPython 3.14.10 `binascii.a2b_base64` filtering/padding acceptance around the existing
native Base64 decoder; it never retains comments or following input lines.
Copyright Python Software Foundation and contributors. The complete upstream
license, including its historical notices, is retained in
[`licenses/PYTHON-LICENSE.txt`](licenses/PYTHON-LICENSE.txt).

- Exact reference source: <https://github.com/python/cpython/blob/v3.14.10/Lib/fnmatch.py>
- Exact Base64 reference: <https://github.com/python/cpython/blob/v3.14.10/Modules/binascii.c>
- License source: <https://github.com/python/cpython/blob/v3.14.10/LICENSE>

Distributions containing the native compatibility implementation retain this
attribution and the complete license alongside the other notices.
