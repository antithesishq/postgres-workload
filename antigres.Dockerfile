FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY src /app/src
COPY Cargo.* /app/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
COPY libvoidstar.so /usr/lib/libvoidstar.so
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY src /app/src
COPY Cargo.* /app/

RUN export VOIDSTAR_PATH="/usr/lib/" \
    && export LD_LIBRARY_PATH="${VOIDSTAR_PATH}" \
    && RUSTFLAGS=" \
    -Ccodegen-units=1 \
    -Cpasses=sancov-module \
    -Cdebuginfo=2 \
    -Cllvm-args=-sanitizer-coverage-level=3 \
    -Cllvm-args=-sanitizer-coverage-trace-pc-guard \
    -Clink-args=-Wl,--build-id \
    -L${VOIDSTAR_PATH} \
    -lvoidstar" \
    cargo build --release

FROM debian:latest AS runtime

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    file \
    postgresql-client \
    && apt-get upgrade -y libc6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/lib/libvoidstar.so /usr/lib/libvoidstar.so
COPY --from=builder /app/target/release/antigres /usr/bin/antigres

# Validate instrumentation
RUN file /usr/bin/antigres | grep -q "not stripped" || \
    (echo "Error: Binary is stripped of symbols" && exit 1)
RUN ldd /usr/bin/antigres | grep "libvoidstar"
RUN nm /usr/bin/antigres | grep "sanitizer_cov_trace_pc_guard"

# Symlink debug symbols for Antithesis symbolization
RUN mkdir /symbols && ln -s /usr/bin/antigres /symbols/antigres

ENTRYPOINT [ "/usr/bin/antigres" ]
