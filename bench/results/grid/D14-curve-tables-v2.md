# D14 curve tables — P-opt vs K-pg

Source: `bench/results/grid` — 72 cell artifacts, 72 publishable.

### P-opt — throughput vs p99

| workload | conns | ops/s | p50 ms | p99 ms | p99.9 ms | hit % | spread % | publishable |
|---|---|---|---|---|---|---|---|---|
| d1024-p1-c1 | 1 | 26,946 | 0.039 | 0.047 | 0.063 | 45.33 | 2.23 | yes |
| d1024-p1-c64 | 64 | 102,218 | 0.631 | 0.687 | 0.783 | 5.14 | 2.24 | yes |
| d1024-p1-c8 | 8 | 99,272 | 0.087 | 0.095 | 0.111 | 17.04 | 1.02 | yes |
| d1024-p16-c1 | 1 | 287,152 | 0.055 | 0.079 | 0.095 | 35.63 | 5.88 | yes |
| d1024-p16-c64 | 64 | 481,075 | 2.175 | 2.431 | 3.855 | 9.45 | 1.33 | yes |
| d1024-p16-c8 | 8 | 423,351 | 0.303 | 0.343 | 0.383 | 39.17 | 3.30 | yes |
| d64-p1-c1 | 1 | 27,770 | 0.039 | 0.047 | 0.063 | 42.97 | 0.74 | yes |
| d64-p1-c64 | 64 | 101,312 | 0.639 | 0.695 | 0.775 | 4.50 | 3.18 | yes |
| d64-p1-c8 | 8 | 101,092 | 0.079 | 0.095 | 0.111 | 12.29 | 0.73 | yes |
| d64-p16-c1 | 1 | 302,251 | 0.055 | 0.071 | 0.095 | 89.73 | 2.07 | yes |
| d64-p16-c64 | 64 | 1,248,212 | 0.815 | 0.903 | 1.095 | 17.02 | 1.25 | yes |
| d64-p16-c8 | 8 | 1,083,054 | 0.119 | 0.143 | 0.159 | 69.87 | 4.61 | yes |

Client configuration per cell, from each cell's committed rerun command:

- `d1024-p1-c1` — data-size=1024 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c64` — data-size=1024 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c8` — data-size=1024 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c1` — data-size=1024 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c64` — data-size=1024 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c8` — data-size=1024 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c1` — data-size=64 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c64` — data-size=64 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c8` — data-size=64 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c1` — data-size=64 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c64` — data-size=64 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c8` — data-size=64 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset

### K-pg — throughput vs p99

| workload | conns | ops/s | p50 ms | p99 ms | p99.9 ms | hit % | spread % | publishable |
|---|---|---|---|---|---|---|---|---|
| d1024-p1-c1 | 1 | 3,312 | 0.279 | 0.599 | 1.679 | 100.00 | 3.09 | yes |
| d1024-p1-c64 | 64 | 10,164 | 4.319 | 28.799 | 43.263 | 33.17 | 1.97 | yes |
| d1024-p1-c8 | 8 | 9,849 | 0.759 | 1.751 | 3.599 | 36.90 | 2.53 | yes |
| d1024-p16-c1 | 1 | 3,916 | 4.063 | 7.007 | 8.575 | 89.29 | 0.78 | yes |
| d1024-p16-c64 | 64 | 10,883 | 92.159 | 162.815 | 201.727 | 31.06 | 1.46 | yes |
| d1024-p16-c8 | 8 | 10,639 | 12.031 | 16.383 | 23.039 | 34.77 | 0.15 | yes |
| d64-p1-c1 | 1 | 3,400 | 0.271 | 0.583 | 0.983 | 98.59 | 4.12 | yes |
| d64-p1-c64 | 64 | 10,214 | 4.287 | 28.543 | 42.495 | 33.01 | 0.69 | yes |
| d64-p1-c8 | 8 | 10,186 | 0.735 | 1.695 | 3.311 | 36.09 | 2.15 | yes |
| d64-p16-c1 | 1 | 4,116 | 3.871 | 5.567 | 7.711 | 86.34 | 1.87 | yes |
| d64-p16-c64 | 64 | 11,171 | 90.111 | 156.671 | 188.415 | 30.20 | 2.61 | yes |
| d64-p16-c8 | 8 | 11,061 | 11.583 | 16.255 | 19.327 | 33.65 | 2.44 | yes |

Client configuration per cell, from each cell's committed rerun command:

- `d1024-p1-c1` — data-size=1024 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c64` — data-size=1024 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p1-c8` — data-size=1024 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c1` — data-size=1024 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c64` — data-size=1024 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d1024-p16-c8` — data-size=1024 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c1` — data-size=64 pipeline=1 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c64` — data-size=64 pipeline=1 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p1-c8` — data-size=64 pipeline=1 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c1` — data-size=64 pipeline=16 clients=1 threads=1 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c64` — data-size=64 pipeline=16 clients=16 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset
- `d64-p16-c8` — data-size=64 pipeline=16 clients=2 threads=4 test-time=60 run-count=3 warmup-keys=200000 client-cpus=4-7 client-pin-mechanism=docker-cpuset

### Same-workload ratios — P-opt vs K-pg (identical client config, arm is the only difference)

| workload | P-opt ops/s | K-pg ops/s | ratio | P-opt p99 | K-pg p99 |
|---|---|---|---|---|---|
| d1024-p1-c1 | 26,946 | 3,312 | **8.1x** | 0.047 ms | 0.599 ms |
| d1024-p1-c64 | 102,218 | 10,164 | **10.1x** | 0.687 ms | 28.799 ms |
| d1024-p1-c8 | 99,272 | 9,849 | **10.1x** | 0.095 ms | 1.751 ms |
| d1024-p16-c1 | 287,152 | 3,916 | **73.3x** | 0.079 ms | 7.007 ms |
| d1024-p16-c64 | 481,075 | 10,883 | **44.2x** | 2.431 ms | 162.815 ms |
| d1024-p16-c8 | 423,351 | 10,639 | **39.8x** | 0.343 ms | 16.383 ms |
| d64-p1-c1 | 27,770 | 3,400 | **8.2x** | 0.047 ms | 0.583 ms |
| d64-p1-c64 | 101,312 | 10,214 | **9.9x** | 0.695 ms | 28.543 ms |
| d64-p1-c8 | 101,092 | 10,186 | **9.9x** | 0.095 ms | 1.695 ms |
| d64-p16-c1 | 302,251 | 4,116 | **73.4x** | 0.071 ms | 5.567 ms |
| d64-p16-c64 | 1,248,212 | 11,171 | **111.7x** | 0.903 ms | 156.671 ms |
| d64-p16-c8 | 1,083,054 | 11,061 | **97.9x** | 0.143 ms | 16.255 ms |

### Matched-p99 comparison — P-opt vs K-pg

Best throughput each arm reaches while holding p99 at or below the budget. **Computed within a single payload size**, never across sizes — comparing one arm at 64 B against the other at 1 KB is not a matched comparison, however good the ratio looks.

**Payload 64 B**

| p99 budget | P-opt ops/s | at | K-pg ops/s | at | ratio P-opt/K-pg |
|---|---|---|---|---|---|
| <= 1 ms | 1,248,212 | d64-p16-c64 | 3,400 | d64-p1-c1 | **367.12x** |
| <= 2 ms | 1,248,212 | d64-p16-c64 | 10,186 | d64-p1-c8 | **122.54x** |
| <= 5 ms | 1,248,212 | d64-p16-c64 | 10,186 | d64-p1-c8 | **122.54x** |
| <= 10 ms | 1,248,212 | d64-p16-c64 | 10,186 | d64-p1-c8 | **122.54x** |
| <= 25 ms | 1,248,212 | d64-p16-c64 | 11,061 | d64-p16-c8 | **112.85x** |
| <= 50 ms | 1,248,212 | d64-p16-c64 | 11,061 | d64-p16-c8 | **112.85x** |

**Payload 1024 B**

| p99 budget | P-opt ops/s | at | K-pg ops/s | at | ratio P-opt/K-pg |
|---|---|---|---|---|---|
| <= 1 ms | 423,351 | d1024-p16-c8 | 3,312 | d1024-p1-c1 | **127.82x** |
| <= 2 ms | 423,351 | d1024-p16-c8 | 9,849 | d1024-p1-c8 | **42.98x** |
| <= 5 ms | 481,075 | d1024-p16-c64 | 9,849 | d1024-p1-c8 | **48.84x** |
| <= 10 ms | 481,075 | d1024-p16-c64 | 9,849 | d1024-p1-c8 | **48.84x** |
| <= 25 ms | 481,075 | d1024-p16-c64 | 10,639 | d1024-p16-c8 | **45.22x** |
| <= 50 ms | 481,075 | d1024-p16-c64 | 10,639 | d1024-p16-c8 | **45.22x** |

### Each-at-own-saturation — P-opt vs K-pg (secondary)

Reported per payload size, for the same reason the matched-p99 table is: a cross-size peak ratio is not a comparison.

**Payload 64 B**

| arm | peak ops/s | at | p99 there | hit % |
|---|---|---|---|---|
| P-opt | 1,248,212 | d64-p16-c64 (64 conns) | 0.903 ms | 17.02 |
| K-pg | 11,171 | d64-p16-c64 (64 conns) | 156.671 ms | 30.20 |

Unconstrained ratio at 64 B: **111.74x** — the two arms are at DIFFERENT latencies here (p99 0.903 ms vs 156.671 ms), which is why the matched-p99 table is the headline and this one is secondary.

**Payload 1024 B**

| arm | peak ops/s | at | p99 there | hit % |
|---|---|---|---|---|
| P-opt | 481,075 | d1024-p16-c64 (64 conns) | 2.431 ms | 9.45 |
| K-pg | 10,883 | d1024-p16-c64 (64 conns) | 162.815 ms | 31.06 |

Unconstrained ratio at 1024 B: **44.20x** — the two arms are at DIFFERENT latencies here (p99 2.431 ms vs 162.815 ms), which is why the matched-p99 table is the headline and this one is secondary.

### Hit-rate parity on paired cells — P-opt vs K-pg

| workload | P-opt hit % | K-pg hit % | difference |
|---|---|---|---|
| d1024-p1-c1 | 45.33 | 100.00 | -54.67 pts |
| d1024-p1-c64 | 5.14 | 33.17 | -28.02 pts |
| d1024-p1-c8 | 17.04 | 36.90 | -19.86 pts |
| d1024-p16-c1 | 35.63 | 89.29 | -53.66 pts |
| d1024-p16-c64 | 9.45 | 31.06 | -21.60 pts |
| d1024-p16-c8 | 39.17 | 34.77 | +4.39 pts |
| d64-p1-c1 | 42.97 | 98.59 | -55.62 pts |
| d64-p1-c64 | 4.50 | 33.01 | -28.50 pts |
| d64-p1-c8 | 12.29 | 36.09 | -23.81 pts |
| d64-p16-c1 | 89.73 | 86.34 | +3.39 pts |
| d64-p16-c64 | 17.02 | 30.20 | -13.17 pts |
| d64-p16-c8 | 69.87 | 33.65 | +36.22 pts |
