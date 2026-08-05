# memtier_benchmark — CLI flags and behavior

**Purpose:** extract exact CLI knobs for running the six-arm benchmark protocol from project-bible §10: ratio 1:10 SET:GET, value sizes 64B/1KB/16KB, pipeline 1 and 16, connections 1/8/64, key space 1M gaussian, 3×60s runs.

---

## Connection and host setup

- `--host=ADDR` or `-h ADDR`: server hostname/IP (default: localhost). memtier_benchmark.1:13
- `--port=PORT` or `-p PORT`: server TCP port (default: 6379). memtier_benchmark.1:18
- Plain TCP (no cluster mode by default). For TCP-only, omit `--cluster-mode`.
- `--authenticate=CREDENTIALS` or `-a CREDENTIALS`: send `AUTH` on connect.
  A bare password for Redis ≤ 5.x semantics (which is what pg_resp implements
  — single password, no ACL users, bible §3.6), or `<USER>:<PASSWORD>` for
  Redis 6+ ACL servers. **Required whenever `pg_resp.password` is set.**
  Added 2026-08-05 after the omission cost a measurement — see the trap below.

---

## Workload shape

- `--ratio=N:M`: SET:GET ratio (default: 1:10). Specifies relative frequency of SET vs GET operations. Format: two colon-separated integers. memtier_benchmark.1:196
  - Example: `--ratio=1:10` means 1 SET for every 10 GETs.

- `--pipeline=N`: concurrent pipelined requests per connection (default: 1). memtier_benchmark.1:199
  - For §10 protocol: separate runs with `--pipeline=1` and `--pipeline=16`.

---

## Data value sizes

Three options exist for size specification; **only one applies per run**:

1. **Single size:** `--data-size=N` or `-d N`: object size in bytes (default: 32). memtier_benchmark.1:360
   - Example: `--data-size=64` for 64 bytes. Run separately for each size.

2. **Range:** `--data-size-range=MIN-MAX`: use random-sized items in the specified range (min-max). memtier_benchmark.1:370
   - Requires `--data-size-pattern=R` (random, default) or `S` (sequential across key range).

3. **Weighted list:** `--data-size-list=SIZE1:WEIGHT1,SIZE2:WEIGHT2,...`: use sizes from weighted distribution. memtier_benchmark.1:373
   - Example: `--data-size-list=64:1,1024:5,16384:4` to weight three sizes.

**For §10 (64B/1KB/16KB):** run three separate executions, each with `--data-size` set to one value (64, 1024, 16384), or use `--data-size-range=64-16384` with probabilistic sizes.

---

## Key space and access pattern

- `--key-maximum=N`: key ID maximum value (default: 10,000,000). memtier_benchmark.1:408
  - For 1M keys: `--key-maximum=1000000`.
  - Keys are generated in range `[--key-minimum, --key-maximum]` (default minimum: 0).

- `--key-pattern=SET_PATTERN:GET_PATTERN`: access pattern policy for SET and GET independently. memtier_benchmark.1:411
  - Formats: `R` (uniform Random, default), `G` (Gaussian), `Z` (zipf), `S` (Sequential), `P` (Parallel/per-client Sequential).
  - Default: `R:R` (random for both).
  - For gaussian distribution (§10): `--key-pattern=G:G`.
  - Pattern string is two characters separated by `:` (first for SET, second for GET).

---

## Test duration and iterations

- `--test-time=SECS`: run duration in seconds (default: none, use `--requests` instead). memtier_benchmark.1:183
  - For §10: `--test-time=60`.

- `--run-count=N` or `-x N`: number of full-test iterations (default: 1). memtier_benchmark.1:68
  - For §10: `--run-count=3` (3 runs × 60s each).

---

## Clients and threads — connection topology (TRAP)

**Critical:** `--clients` and `--threads` **compose multiplicatively** to total connections.

- `--clients=N` or `-c N`: number of clients **per thread** (default: 50). memtier_benchmark.1:177
- `--threads=N` or `-t N`: number of threads (default: 4). memtier_benchmark.1:180
- **Total connections = `--clients` × `--threads`**.

**Example:** `--clients=1 --threads=1` → 1 total connection. `--clients=8 --threads=8` → 64 total connections.

**For §10 (connections 1/8/64):** run three variants:
  - 1 connection: `--clients=1 --threads=1`
  - 8 connections: `--clients=8 --threads=1` (or any product equaling 8)
  - 64 connections: `--clients=8 --threads=8` (or `--clients=64 --threads=1`, etc.)

---

## Output and latency percentiles

- `--print-percentiles=P1,P2,...`: comma-separated latency percentiles to display (default: 50,99,99.9). memtier_benchmark.1:117
  - Percentiles are specified as raw numbers (e.g., `50`, `99`, `99.9`, `99.99`), not ratios.
  - Default reports **p50, p99, p99.9** (shown in the summary table by default).
  - Detailed latency histogram printed automatically unless `--hide-histogram` is set.

**Latency reporting:** percentiles are printed in the summary table as columns (e.g., "p50 Latency", "p99 Latency"). Histogram format uses HdrHistogram library (t-digest alternative); both text and `.hgrm` formats available via `--hdr-file-prefix=PREFIX`. memtier_benchmark.1:108

---

## Operations/sec and throughput reporting

- **Ops/sec:** calculated as `(total_ops / test_duration_usec) × 1,000,000` and printed in the summary table under "Ops/sec" column (one row per command type: Sets, Gets, Totals). run_stats.cpp:1196

- **Throughput:** bytes-per-sec (`KB/sec`) is also a column in the summary table. memtier_benchmark.1:114 references a print_kb_sec_column function.

**Memory/RAM reporting:** memtier_benchmark **does NOT report server-side memory stats** (e.g., RSS, used memory from INFO). Operator must measure server RAM separately (e.g., `redis-cli INFO memory` for Redis/Valkey, or `resp.stats()` for pg_resp, or OS-level tools). memtier_benchmark only tracks client-side metrics.

---

## Summary example: complete command for one arm

```bash
memtier_benchmark \
  --host=127.0.0.1 --port=6379 \
  --ratio=1:10 \
  --data-size=1024 \
  --pipeline=16 \
  --clients=8 --threads=8 \
  --key-pattern=G:G --key-maximum=1000000 \
  --test-time=60 \
  --run-count=3 \
  --print-percentiles=50,99,99.9
```

Runs 3 iterations of 60 seconds each, with 64 total connections (8 clients × 8 threads), 1:10 SET:GET ratio, 1 KB values, pipeline=16, gaussian key access across 1M keys. Reports p50/p99/p99.9 latency and ops/sec in the summary table.

---

## Traps and gotchas

1. **Clients × threads:** most first-time users expect `--clients=64` to mean 64 total connections. It does not; it means 64 clients *per thread*. If `--threads` defaults to 4, that's 256 connections. Always compute: total = clients × threads.

2. **Percentile syntax:** `--print-percentiles=99.9` not `--print-percentiles=0.999`. The flag takes raw percentile values (50, 99, 99.9, 99.99), not fractions.

3. **Data size options are mutually exclusive:** `--data-size`, `--data-size-range`, and `--data-size-list` are alternatives. Using more than one may cause parsing errors or undefined behavior. Choose one.

4. **Key pattern string format:** `--key-pattern=G:G` is a two-character string with colon separator. Single-character patterns like `--key-pattern=G` (no colon) are invalid. First character is SET pattern, second is GET pattern.

5. **Default key range starts at 0:** `--key-minimum=0` is implicit. If you specify `--key-maximum=1000000`, you're using keys [0, 1000000], not [1, 1000000].

6. **No server memory stats in memtier output:** memtier reports *client*-side throughput and latency. Server RAM consumption must be measured by the operator outside memtier (e.g., `pidstat`, `INFO memory`, or application-level stats). Do not expect memtier to report "Server Memory Used".

7. **Default test termination:** if neither `--test-time` nor `--requests` is specified, `--requests=10000` (default) applies. For long-running benchmarks, always set `--test-time`.

8. **Run-count vs parallel runs:** `--run-count=3` runs 3 sequential full benchmarks, not 3 parallel iterations. Total time ≈ 3 × 60 s = 180 s. Results are reported per-run; use `--print-all-runs` to output all three, otherwise only the median is shown.

9. **A run against an auth-enabled server silently benchmarks *rejection*, and inflates its own output ~1000×.** memtier does **not** fail, warn, or exit when every command comes back `-NOAUTH Authentication required.` It reports a perfectly normal-looking ops/sec table — of refusals. Discovered 2026-08-05 (Phase 3 PRE-STEP): two saturation runs against a pg_resp instance with `pg_resp.password` set produced plausible 158k/176k ops/sec figures that measured nothing but the cost of saying no, and 820 MB of raw output consisting of 5.3 million repetitions of one error line. Two tells, both cheap to check:
   - `SHOW pg_resp.password;` **before** the run — if non-empty, pass `--authenticate`.
   - `grep -c NOAUTH <output>` **after** the run — must be 0.
   The giveaway in the summary table is a 0% hit rate that does not improve no matter how long the run goes: if no GET ever succeeds, no GET was ever executed. Full write-up in `bench/results/ENV.md` §4.

---

## File pointers

- Man page: `/home/claudiu/pg_resp/pg_resp/ref/memtier_benchmark/memtier_benchmark.1`
- Config types (ratio/range parsing): `/home/claudiu/pg_resp/pg_resp/ref/memtier_benchmark/config_types.cpp`
- Default percentiles (50, 99, 99.9): `/home/claudiu/pg_resp/pg_resp/ref/memtier_benchmark/memtier_benchmark.cpp:978`
- Ops/sec calculation: `/home/claudiu/pg_resp/pg_resp/ref/memtier_benchmark/run_stats.cpp:1196`
- README with flag examples: `/home/claudiu/pg_resp/pg_resp/ref/memtier_benchmark/README.md`
