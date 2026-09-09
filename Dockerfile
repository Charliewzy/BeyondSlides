FROM rust:1.94-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang \
        cmake \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
    && strip target/release/beyond-slides \
    && mkdir -p /opt/beyond-slides \
    && cp target/release/beyond-slides /opt/beyond-slides/beyond-slides \
    && /opt/beyond-slides/beyond-slides install-runtime-tools /opt/beyond-slides/runtime-tools

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libgcc-s1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 beyondslides \
    && mkdir -p /data \
    && chown beyondslides:beyondslides /data

COPY --from=builder /opt/beyond-slides/beyond-slides /opt/beyond-slides/beyond-slides
COPY --from=builder /opt/beyond-slides/runtime-tools /opt/beyond-slides/runtime-tools

ENV BEYOND_SLIDES_BIND_ADDRESS=0.0.0.0
WORKDIR /data
VOLUME ["/data"]
EXPOSE 7842
USER beyondslides

ENTRYPOINT ["/opt/beyond-slides/beyond-slides"]
CMD ["serve", "/data", "7842"]
