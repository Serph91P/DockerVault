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
