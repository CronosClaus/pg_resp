# D14 curve tables — P-opt vs K-pg

Source: `bench/results/stage-a` — 18 cell artifacts, 18 publishable.

### P-opt — throughput vs p99

| workload | conns | ops/s | p50 ms | p99 ms | p99.9 ms | hit % | spread % | publishable |
|---|---|---|---|---|---|---|---|---|
| d1024-p1-c1 | 1 | 22,412 | 0.047 | 0.063 | 0.079 | 43.14 | 0.31 | yes |
| d1024-p1-c64 | 64 | 93,612 | 0.687 | 0.735 | 0.839 | 100.00 | 0.50 | yes |
| d1024-p1-c8 | 8 | 85,889 | 0.095 | 0.119 | 0.327 | 58.19 | 0.69 | yes |
| d1024-p16-c1 | 1 | 257,509 | 0.063 | 0.087 | 0.103 | 36.16 | 1.52 | yes |
| d1024-p16-c32 | 32 | 398,178 | 1.287 | 1.367 | 2.879 | 55.14 | 0.39 | yes |
| d1024-p16-c64 | 64 | 401,204 | 2.559 | 2.687 | 3.887 | 84.66 | 1.28 | yes |
| d1024-p16-c8 | 8 | 359,004 | 0.359 | 0.415 | 0.479 | 39.27 | 2.26 | yes |
| d64-p16-c32 | 32 | 1,041,676 | 0.487 | 0.559 | 0.655 | 99.62 | 0.58 | yes |
| d64-p16-c8 | 8 | 860,960 | 0.151 | 0.175 | 0.199 | 99.87 | 3.86 | yes |

Client configuration per cell, from each cell's committed rerun command:

- `d1024-p1-c1` — data-size=1024 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c64` — data-size=1024 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c8` — data-size=1024 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c1` — data-size=1024 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c32` — data-size=1024 pipeline=16 clients=8 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c64` — data-size=1024 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c8` — data-size=1024 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c32` — data-size=64 pipeline=16 clients=8 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c8` — data-size=64 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset

### K-pg — throughput vs p99

| workload | conns | ops/s | p50 ms | p99 ms | p99.9 ms | hit % | spread % | publishable |
|---|---|---|---|---|---|---|---|---|
| d1024-p1-c1 | 1 | 3,334 | 0.271 | 0.679 | 0.887 | 63.91 | 2.38 | yes |
| d1024-p1-c64 | 64 | 8,978 | 4.863 | 32.639 | 49.151 | 66.13 | 2.05 | yes |
| d1024-p1-c8 | 8 | 9,376 | 0.791 | 1.919 | 3.151 | 63.38 | 0.25 | yes |
| d1024-p16-c1 | 1 | 3,701 | 4.319 | 7.199 | 8.383 | 63.79 | 0.68 | yes |
| d1024-p16-c32 | 32 | 9,815 | 51.199 | 86.527 | 108.031 | 61.71 | 1.36 | yes |
| d1024-p16-c64 | 64 | 9,909 | 101.375 | 178.175 | 218.111 | 61.01 | 2.73 | yes |
| d1024-p16-c8 | 8 | 9,961 | 12.799 | 17.535 | 24.447 | 62.40 | 0.36 | yes |
| d64-p16-c32 | 32 | 10,123 | 49.919 | 83.455 | 98.303 | 62.65 | 2.24 | yes |
| d64-p16-c8 | 8 | 10,296 | 12.415 | 17.535 | 21.503 | 63.63 | 0.99 | yes |

Client configuration per cell, from each cell's committed rerun command:

- `d1024-p1-c1` — data-size=1024 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c64` — data-size=1024 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c8` — data-size=1024 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c1` — data-size=1024 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c32` — data-size=1024 pipeline=16 clients=8 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c64` — data-size=1024 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c8` — data-size=1024 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c32` — data-size=64 pipeline=16 clients=8 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c8` — data-size=64 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-time=60 client-cpus=4-7 client-pin-mechanism=docker-cpuset

### Same-workload ratios — P-opt vs K-pg (identical client config, arm is the only difference)

| workload | P-opt ops/s | K-pg ops/s | ratio | P-opt p99 | K-pg p99 |
|---|---|---|---|---|---|
| d1024-p1-c1 | 22,412 | 3,334 | **6.7x** | 0.063 ms | 0.679 ms |
| d1024-p1-c64 | 93,612 | 8,978 | **10.4x** | 0.735 ms | 32.639 ms |
| d1024-p1-c8 | 85,889 | 9,376 | **9.2x** | 0.119 ms | 1.919 ms |
| d1024-p16-c1 | 257,509 | 3,701 | **69.6x** | 0.087 ms | 7.199 ms |
| d1024-p16-c32 | 398,178 | 9,815 | **40.6x** | 1.367 ms | 86.527 ms |
| d1024-p16-c64 | 401,204 | 9,909 | **40.5x** | 2.687 ms | 178.175 ms |
| d1024-p16-c8 | 359,004 | 9,961 | **36.0x** | 0.415 ms | 17.535 ms |
| d64-p16-c32 | 1,041,676 | 10,123 | **102.9x** | 0.559 ms | 83.455 ms |
| d64-p16-c8 | 860,960 | 10,296 | **83.6x** | 0.175 ms | 17.535 ms |

### Matched-p99 comparison — P-opt vs K-pg

Best throughput each arm reaches while holding p99 at or below the budget. **Computed within a single payload size**, never across sizes — comparing one arm at 64 B against the other at 1 KB is not a matched comparison, however good the ratio looks.

**Payload 64 B**

| p99 budget | P-opt ops/s | at | K-pg ops/s | at | ratio P-opt/K-pg |
|---|---|---|---|---|---|
| <= 1 ms | 1,041,676 | d64-p16-c32 | — | — | K-pg meets no cell at this budget |
| <= 2 ms | 1,041,676 | d64-p16-c32 | — | — | K-pg meets no cell at this budget |
| <= 5 ms | 1,041,676 | d64-p16-c32 | — | — | K-pg meets no cell at this budget |
| <= 10 ms | 1,041,676 | d64-p16-c32 | — | — | K-pg meets no cell at this budget |
| <= 25 ms | 1,041,676 | d64-p16-c32 | 10,296 | d64-p16-c8 | **101.17x** |
| <= 50 ms | 1,041,676 | d64-p16-c32 | 10,296 | d64-p16-c8 | **101.17x** |

**Payload 1024 B**

| p99 budget | P-opt ops/s | at | K-pg ops/s | at | ratio P-opt/K-pg |
|---|---|---|---|---|---|
| <= 1 ms | 359,004 | d1024-p16-c8 | 3,334 | d1024-p1-c1 | **107.70x** |
| <= 2 ms | 398,178 | d1024-p16-c32 | 9,376 | d1024-p1-c8 | **42.47x** |
| <= 5 ms | 401,204 | d1024-p16-c64 | 9,376 | d1024-p1-c8 | **42.79x** |
| <= 10 ms | 401,204 | d1024-p16-c64 | 9,376 | d1024-p1-c8 | **42.79x** |
| <= 25 ms | 401,204 | d1024-p16-c64 | 9,961 | d1024-p16-c8 | **40.28x** |
| <= 50 ms | 401,204 | d1024-p16-c64 | 9,961 | d1024-p16-c8 | **40.28x** |

### Each-at-own-saturation — P-opt vs K-pg (secondary)

Reported per payload size, for the same reason the matched-p99 table is: a cross-size peak ratio is not a comparison.

**Payload 64 B**

| arm | peak ops/s | at | p99 there | hit % |
|---|---|---|---|---|
| P-opt | 1,041,676 | d64-p16-c32 (32 conns) | 0.559 ms | 99.62 |
| K-pg | 10,296 | d64-p16-c8 (8 conns) | 17.535 ms | 63.63 |

Unconstrained ratio at 64 B: **101.17x** — the two arms are at DIFFERENT latencies here (p99 0.559 ms vs 17.535 ms), which is why the matched-p99 table is the headline and this one is secondary.

**Payload 1024 B**

| arm | peak ops/s | at | p99 there | hit % |
|---|---|---|---|---|
| P-opt | 401,204 | d1024-p16-c64 (64 conns) | 2.687 ms | 84.66 |
| K-pg | 9,961 | d1024-p16-c8 (8 conns) | 17.535 ms | 62.40 |

Unconstrained ratio at 1024 B: **40.28x** — the two arms are at DIFFERENT latencies here (p99 2.687 ms vs 17.535 ms), which is why the matched-p99 table is the headline and this one is secondary.

### Hit-rate parity on paired cells — P-opt vs K-pg

| workload | P-opt hit % | K-pg hit % | difference |
|---|---|---|---|
| d1024-p1-c1 | 43.14 | 63.91 | -20.77 pts |
| d1024-p1-c64 | 100.00 | 66.13 | +33.87 pts |
| d1024-p1-c8 | 58.19 | 63.38 | -5.19 pts |
| d1024-p16-c1 | 36.16 | 63.79 | -27.63 pts |
| d1024-p16-c32 | 55.14 | 61.71 | -6.58 pts |
| d1024-p16-c64 | 84.66 | 61.01 | +23.65 pts |
| d1024-p16-c8 | 39.27 | 62.40 | -23.13 pts |
| d64-p16-c32 | 99.62 | 62.65 | +36.97 pts |
| d64-p16-c8 | 99.87 | 63.63 | +36.23 pts |
