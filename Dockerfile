ARG IMAGE_RUST_VERSION=1.98.0-alpine3.24
ARG IMAGE_RUST_DIGEST=a10e64dd139b7387337c7fbe8aca31b959b57b2fd4c8ae20a02cf1d6ea424dce

ARG UID=65532
ARG GID=65532

FROM docker.io/rust:${IMAGE_RUST_VERSION}@sha256:${IMAGE_RUST_DIGEST} AS builder

RUN set -e && \
    apk add --no-cache \
    make=4.4.1-r4 \
    musl-dev=1.2.6-r2

RUN set -e && \
    rm -rf /var/lib/apk/tmp/* /var/cache/apk/* /var/log/apk.log

WORKDIR /src

COPY . .

ARG TARGETARCH

RUN set -e && \
    case ${TARGETARCH} in \
    "amd64")  TARGET="x86_64-unknown-linux-musl" \
    ;; \
    "arm64")  TARGET="aarch64-unknown-linux-musl" \
    ;; \
    *)        echo "Unsupported architecture: ${TARGETARCH}"; exit 1; \
    esac \
    && \
    cargo build --locked --release --package tespeed-bin --target ${TARGET} && \
    mkdir -p build && \
    cp target/${TARGET}/release/tespeed ./build/tespeed

FROM scratch

ARG IMAGE_VCS_DATE
ARG IMAGE_VCS_REV

ARG IMAGE_TESPEED_VERSION

LABEL org.opencontainers.image.title="tespeed" \
    org.opencontainers.image.vendor="Hantong Chen" \
    org.opencontainers.image.authors="Hantong Chen" \
    org.opencontainers.image.description="OCI image for tespeed" \
    org.opencontainers.image.documentation="https://github.com/hanyu-dev/tespeed/blob/main/README.md" \
    org.opencontainers.image.source="https://github.com/hanyu-dev/tespeed" \
    org.opencontainers.image.url="https://github.com/hanyu-dev/tespeed" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.created=${IMAGE_VCS_DATE} \
    org.opencontainers.image.version=${IMAGE_TESPEED_VERSION} \
    org.opencontainers.image.revision=${IMAGE_VCS_REV}

ARG UID
ARG GID

COPY --from=builder --chown="${UID}:${GID}" --chmod=775 /src/build/tespeed /opt/tespeed/tespeed

WORKDIR /opt/tespeed

USER ${UID}:${GID}

EXPOSE 1585

ENTRYPOINT ["/opt/tespeed/tespeed"]
