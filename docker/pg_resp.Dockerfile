# Builds pg_resp into a real postgres:18 image. UNTESTED — this environment
# has no docker at all (confirmed in reports/phase0.md's environment
# pre-flight), so this file has never been built. Grounded in one verified
# fact: `cargo pgrx package` was run locally against this machine's
# pgrx-managed pg18 and its output mirrors the target pg_config's install
# prefix exactly (e.g. a pgrx-managed prefix produced
# target/release/pg_resp-pg18/home/.../pgrx-install/lib/postgresql/pg_resp.so)
# — against a real Debian/Ubuntu postgresql-18 package (as here, via apt),
# that prefix is /usr/lib/postgresql/18 and /usr/share/postgresql/18, which
# is what the COPY lines below assume. Verify this on first real build.
FROM postgres:18 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl build-essential libclang-dev postgresql-server-dev-18 \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rust \
    CARGO_HOME=/opt/rust \
    PATH=/opt/rust/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
RUN cargo install cargo-pgrx --locked --version 0.19.2
RUN cargo pgrx init --pg18 "$(command -v pg_config)"

WORKDIR /build
COPY . .
RUN cd crates/pg_resp && cargo pgrx package --pg-config "$(command -v pg_config)"

FROM postgres:18
COPY --from=builder /build/crates/pg_resp/target/release/pg_resp-pg18/usr/lib/postgresql/18/lib/pg_resp.so \
    /usr/lib/postgresql/18/lib/pg_resp.so
COPY --from=builder /build/crates/pg_resp/target/release/pg_resp-pg18/usr/share/postgresql/18/extension/ \
    /usr/share/postgresql/18/extension/
RUN echo "shared_preload_libraries = 'pg_resp.so'" >> /usr/share/postgresql/postgresql.conf.sample
EXPOSE 5432 6379
