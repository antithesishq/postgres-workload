FROM debian:bookworm-slim AS builder

ARG PG_VERSION=18.3
ARG ENABLE_ASSERTIONS=

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        clang-16 \
        llvm-16 \
        lld-16 \
        bison \
        flex \
        libreadline-dev \
        zlib1g-dev \
        libssl-dev \
        libicu-dev \
        pkg-config \
        wget \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN update-alternatives --install /usr/bin/clang   clang   /usr/bin/clang-16   100 && \
    update-alternatives --install /usr/bin/clang++ clang++ /usr/bin/clang++-16 100 && \
    update-alternatives --install /usr/bin/ld.lld  ld.lld  /usr/bin/ld.lld-16  100

COPY libvoidstar.so /usr/lib/libvoidstar.so
RUN ldconfig

RUN wget -q -O /tmp/postgresql.tar.bz2 \
        "https://ftp.postgresql.org/pub/source/v${PG_VERSION}/postgresql-${PG_VERSION}.tar.bz2" && \
    mkdir -p /usr/src/postgresql && \
    tar xjf /tmp/postgresql.tar.bz2 -C /usr/src/postgresql --strip-components=1 && \
    rm /tmp/postgresql.tar.bz2

WORKDIR /usr/src/postgresql

RUN CC=clang \
    CFLAGS="-O2 -g -fsanitize-coverage=trace-pc-guard -fno-sanitize-link-runtime" \
    LDFLAGS="-Wl,--build-id -fuse-ld=lld -L/usr/lib -lvoidstar" \
    ./configure \
        --prefix=/usr/local/pgsql \
        --with-openssl \
        --with-icu \
        ${ENABLE_ASSERTIONS:+--enable-cassert}

RUN make -j"$(nproc)" && make install

RUN cd contrib && make -j"$(nproc)" && make install

RUN echo "=== Instrumentation validation ===" && \
    echo "--- coverage callback symbols (expect U) ---" && \
    nm -D /usr/local/pgsql/bin/postgres | grep "sanitizer_cov_trace_pc_guard" && \
    nm -D /usr/local/pgsql/bin/postgres | grep -q "U __sanitizer_cov_trace_pc_guard" && \
    nm -D /usr/local/pgsql/bin/postgres | grep -q "U __sanitizer_cov_trace_pc_guard_init" && \
    echo "PASS: coverage callbacks are undefined (U) — will resolve from libvoidstar" && \
    echo "--- runtime dependency on libvoidstar ---" && \
    ldd /usr/local/pgsql/bin/postgres | grep "libvoidstar" && \
    echo "PASS: libvoidstar.so is a runtime dependency" && \
    echo "=== All checks passed ==="

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        libreadline8 \
        zlib1g \
        libssl3 \
        libicu72 \
        gosu \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/pgsql /usr/local/pgsql

COPY --from=builder /usr/lib/libvoidstar.so /usr/lib/libvoidstar.so
RUN ldconfig

RUN mkdir -p /symbols && \
    ln -s /usr/local/pgsql/bin/postgres /symbols/postgres && \
    for f in /usr/local/pgsql/lib/*.so*; do \
        [ -f "$f" ] && ln -s "$f" /symbols/; \
    done && \
    for f in /usr/local/pgsql/bin/*; do \
        [ -f "$f" ] && [ -x "$f" ] && ln -sf "$f" /symbols/; \
    done

ENV PATH="/usr/local/pgsql/bin:${PATH}"
ENV PGDATA="/var/lib/postgresql/data"

RUN groupadd -r postgres && useradd -r -g postgres -d /var/lib/postgresql -s /bin/bash postgres && \
    mkdir -p "$PGDATA" && chown -R postgres:postgres "$PGDATA" && \
    mkdir -p /run/postgresql && chown -R postgres:postgres /run/postgresql

COPY <<'ENTRYPOINT' /usr/local/bin/docker-entrypoint.sh
#!/usr/bin/env bash
set -e

if [ "$(id -u)" = '0' ]; then
    # If PGDATA is empty, initialise the cluster.
    if [ ! -s "$PGDATA/PG_VERSION" ]; then
        gosu postgres initdb --username=postgres
        # Allow connections from any address (useful in Antithesis network).
        echo "host all all 0.0.0.0/0 trust" >> "$PGDATA/pg_hba.conf"
        echo "listen_addresses = '*'" >> "$PGDATA/postgresql.conf"
    fi
    exec gosu postgres postgres "$@"
fi

exec postgres "$@"
ENTRYPOINT

RUN chmod +x /usr/local/bin/docker-entrypoint.sh

EXPOSE 5432

ENTRYPOINT ["docker-entrypoint.sh"]
