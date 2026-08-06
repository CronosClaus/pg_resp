# Builds memtier_benchmark at the pinned commit (docs/refs/PINS.md:
# 272eeb647df5) into a runnable image.
#
# WHY THIS EXISTS
# ---------------
# The official benchmark box is frozen as bootstrapped: docker, git, python3,
# tmux, rsync and nothing else — no compiler, no apt packages added (Phase 4
# box requirement 5). memtier_benchmark therefore cannot be built on the host,
# and the development machine's native binary must not simply be copied over,
# because it was built against a different distribution's libevent/OpenSSL.
#
# Building it here from the SAME pinned commit the development runs used keeps
# the client identical across both environments, which is the point of the pin:
# a client version change moves throughput numbers on its own.
#
# The image is run with --network host so the client reaches every arm over the
# host's loopback, exactly as a native binary would — no NAT, no userland
# proxy. See bench/harness/arms.sh's header for why that matters.
FROM ubuntu:24.04

# Pinned toolchain-free build deps. libpcre3-dev (not libpcre2) is what
# memtier's configure.ac looks for.
# libtool is REQUIRED and was missing: `autoreconf -ivf` shells out to
# `libtoolize`, which ships in the libtool package. Its absence built fine on the
# benchmark box and failed on a clean build elsewhere with
# "Can't exec libtoolize" — the image happened to work there because of a
# differently-dated package index, not because the dependency was satisfied.
# Found by rebuilding this image from scratch on a second machine, which is the
# only thing that finds a missing build dependency.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential autoconf automake libtool pkg-config ca-certificates \
      libevent-dev libpcre3-dev zlib1g-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
# Build context is the pinned reference clone (ref/memtier_benchmark).
COPY . .
RUN autoreconf -ivf && ./configure && make -j"$(nproc)" && make install

# Fail the build rather than the benchmark if the binary cannot even report
# its own version — a client that will not start is better discovered here.
RUN memtier_benchmark --version

ENTRYPOINT []
CMD ["memtier_benchmark", "--help"]
