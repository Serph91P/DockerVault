FROM rust@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --locked --release --jobs 2 -p dockervault-tools --bin dockervault-recover \
    && mkdir /licenses \
    && cp /usr/local/cargo/registry/src/*/fernet-0.2.2/LICENSE /licenses/fernet-MPL-2.0.txt

FROM debian@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source="https://github.com/Serph91P/DockerVault" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.description="Native recovery of existing DockerVault archive and age sidecar formats"
RUN apt-get update && apt-get install --yes --no-install-recommends libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir /work && chown 1000:1000 /work
COPY --from=build /source/target/release/dockervault-recover /usr/local/bin/dockervault-recover
COPY --from=build /licenses /usr/share/licenses/dockervault
COPY docs/RUST_DEPENDENCY_NOTICES.md /usr/share/licenses/dockervault/DEPENDENCIES.md
COPY docs/licenses/PYTHON-LICENSE.txt /usr/share/licenses/dockervault/PYTHON-LICENSE.txt
WORKDIR /work
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/dockervault-recover"]
