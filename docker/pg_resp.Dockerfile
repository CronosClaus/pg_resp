# Builds pg_resp into a real postgres:18 image.
#
# Built and fixed for real once docker became available (2026-08-05):
# 1. Piping curl straight into `sh` masked a cert-store failure (postgres:18
#    has no ca-certificates package by default) — sh saw empty stdin and
#    exited 0, so rustup silently never installed. Fixed: fetch to a file
#    first, ca-certificates added.
# 2. cargo-pgrx's build needs pkg-config + libssl-dev (openssl-sys) — not
#    pulled in by build-essential/libclang-dev alone. Added.
# 3. pg_resp is a cargo WORKSPACE member (crates/resp-proto, crates/resp-store
#    are siblings) — `cargo pgrx package`'s output therefore lands in the
#    *workspace-root* target/ dir, not crates/pg_resp/target/, even though
#    the command is run from inside crates/pg_resp/. COPY paths below were
#    originally guessed at the per-crate path (grounded in an earlier local
#    `cargo pgrx package` run whose "no such file" result was misattributed
#    to a shell-cwd quirk instead of this) — fixed to the workspace-root path.
FROM postgres:18 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential libclang-dev postgresql-server-dev-18 \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rust \
    CARGO_HOME=/opt/rust \
    PATH=/opt/rust/bin:$PATH
# Piping curl straight into sh masks a curl failure (sh just sees empty
# stdin and exits 0) — fetch to a file first so a broken download fails loudly.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --profile minimal \
    && rm /tmp/rustup.sh
RUN cargo install cargo-pgrx --locked --version 0.19.2
RUN cargo pgrx init --pg18 "$(command -v pg_config)"

WORKDIR /build
COPY . .
RUN cd crates/pg_resp && cargo pgrx package --pg-config "$(command -v pg_config)"

FROM postgres:18
COPY --from=builder /build/target/release/pg_resp-pg18/usr/lib/postgresql/18/lib/pg_resp.so \
    /usr/lib/postgresql/18/lib/pg_resp.so
COPY --from=builder /build/target/release/pg_resp-pg18/usr/share/postgresql/18/extension/ \
    /usr/share/postgresql/18/extension/
RUN echo "shared_preload_libraries = 'pg_resp.so'" >> /usr/share/postgresql/postgresql.conf.sample
EXPOSE 5432 6379
