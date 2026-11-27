# Built immediately before this stage from the same checkout/architecture.
ARG RUST_DEV_IMAGE=dockervault-rust-ci
FROM ${RUST_DEV_IMAGE}
# Same pinned release/checksums as the accepted Python runtime; no Python extractor.
RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) arch=amd64; checksum=b5c9b2fb6ada8a400c5fc5d48cd112dc1adea21a3b73b03857059374dd8a78d0 ;; \
      arm64) arch=arm64; checksum=66ce9c7fbdf6ba38991fa2ac193ed051bd6d04aeec693900c848154bf549484f ;; \
      *) exit 1 ;; \
    esac; \
    archive="rclone-v1.72.1-linux-${arch}"; \
    curl --fail --silent --show-error --location \
      "https://downloads.rclone.org/v1.72.1/${archive}.zip" --output target/tmp/rclone.zip; \
    printf '%s  %s\n' "$checksum" target/tmp/rclone.zip | sha256sum --check -; \
    unzip -q target/tmp/rclone.zip -d target/tmp/rclone-extracted; \
    install -m 0755 "target/tmp/rclone-extracted/${archive}/rclone" /workspace/rclone; \
    rm -rf target/tmp/rclone.zip target/tmp/rclone-extracted
RUN cargo build --bin dockervault-process-guard --examples --locked --offline
CMD ["/bin/sh", "-ec", "target/debug/examples/rclone_capabilities /workspace/target/debug/dockervault-process-guard /workspace/rclone; target/debug/examples/rclone_managed /workspace/target/debug/dockervault-process-guard /workspace/rclone"]
