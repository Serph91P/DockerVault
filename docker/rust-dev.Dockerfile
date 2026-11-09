FROM rust@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c
RUN rustup component add rustfmt clippy && cargo install --locked cargo-audit --version 0.22.2 && cargo install --locked cargo-deny --version 0.20.2
RUN mkdir -p /workspace/target/tmp && chown -R 1000:1000 /workspace
ENV CARGO_HOME=/workspace/.cargo TMPDIR=/workspace/target/tmp
WORKDIR /workspace
USER 1000:1000
COPY --chown=1000:1000 Cargo.toml Cargo.lock rust-toolchain.toml deny.toml ./
COPY --chown=1000:1000 crates ./crates
RUN cargo fetch --locked
CMD ["cargo", "test", "--workspace", "--locked", "--offline"]
