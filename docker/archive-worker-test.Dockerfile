# Isolated privilege-contract test image, not an application or release image.
FROM rust@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --locked --release --jobs 2 -p dockervault-tools --bin dockervault-archive-worker --example archive_worker_privileges \
    && cargo build --locked --release --jobs 2 -p dockervault-service --example privileged_archive_supervision

FROM debian@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update && apt-get install --yes --no-install-recommends sudo libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --no-create-home dockervault \
    && mkdir /backups /fixture /fixture/source /fixture/large \
    && truncate -s 1G /fixture/large/data \
    && chown 999:999 /fixture/large/data && chmod 0400 /fixture/large/data \
    && chown 1000:1000 /backups \
    && printf 'PUBLIC root-readable fixture\n' > /fixture/source/data \
    && chown 999:999 /fixture/source/data && chmod 0400 /fixture/source/data \
    && printf 'dockervault ALL=(root) NOPASSWD: /usr/local/libexec/dockervault-archive-worker ""\n' > /etc/sudoers.d/dockervault-archive-worker \
    && chmod 0440 /etc/sudoers.d/dockervault-archive-worker \
    && visudo -cf /etc/sudoers.d/dockervault-archive-worker
COPY --from=build /source/target/release/dockervault-archive-worker /usr/local/libexec/dockervault-archive-worker
COPY --from=build /source/target/release/examples/archive_worker_privileges /usr/local/bin/archive-worker-privileges
COPY --from=build /source/target/release/examples/privileged_archive_supervision /usr/local/bin/privileged-archive-supervision
USER 1000:1000
ENTRYPOINT ["/usr/local/libexec/dockervault-archive-worker"]
