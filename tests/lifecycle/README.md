# tests/lifecycle

The harnesses that keep pg_resp's failure behaviour honest. Bible §9 requires
the Phase 0 spike-S1 lifecycle table to live in CI "forever"; these scripts are
that, plus the two Phase 3 additions.

| script | what it proves | when to run |
|---|---|---|
| `lifecycle.py` | The S1 gate table: PING answers; `pg_ctl stop -m fast` exits in < 2s leaving no orphan and releasing the port; 20 start/stop cycles rebind the port every time; `kill -9` on the worker recovers with no intervention. | after any change touching worker lifecycle, signals, or the listener |
| `slow_reader.py` | ADD1's partial-write fix: large pipelined replies to a slow reader arrive intact and in order, and a bystander connection is not head-of-line blocked. | after any change to the write path |
| `panic_policy.py` | S6: a per-connection panic costs exactly one connection; a top-level panic exits the worker, which restarts itself; and the worker's FATAL exit does **not** force cluster-wide crash recovery. Requires `--features debug_panic`. | after any change to the fences, the watchdog, or shutdown |

## Running them

All three drive a real instance; none of them can be a `#[pg_test]`, because
the behaviour they test only exists when a real bgworker is listening on a real
port (pgrx's test harness does not preload the library).

```bash
PGBIN=~/.pgrx/18.4/pgrx-install/bin

python3 tests/lifecycle/lifecycle.py --pgbin $PGBIN --datadir ~/.pgrx/data-18 \
    --pg-port 28818 --password secret123

python3 tests/lifecycle/slow_reader.py --password secret123

# panic_policy.py needs the failure-injection commands, which are NOT in a
# default build:
cargo pgrx install --pg-config $PGBIN/pg_config --no-default-features \
    --features pg18,debug_panic
python3 tests/lifecycle/panic_policy.py --psql $PGBIN/psql --pg-port 28818 \
    --password secret123 --logfile /path/to/postgres.log
# ...then reinstall WITHOUT debug_panic.
```

Omit `--password` if `pg_resp.password` is not set on the instance.

**If psql reports `connection to server on socket "/tmp/.s.PGSQL.28818" failed:
No such file or directory` while the server is demonstrably running**, the
instance was started by `cargo pgrx start`/`cargo pgrx test`, which sets
`unix_socket_directories=~/.pgrx` — a plain `pg_ctl start` uses
postgresql.conf's default instead (`/tmp` here). Pass the directory explicitly:
`--pg-host ~/.pgrx`. All three harnesses accept `--pg-host`. Same trap as
`pgrx-patterns` §8.6.

## Measured results, Phase 3

`lifecycle.py`: stop in **0.20s** (gate < 2s), 20/20 start-stop cycles clean, no
orphans, port released every time, SIGKILL recovery in **0.6s**.

`slow_reader.py`: 64 x 64KB pipelined replies to a client with
`SO_RCVBUF=4096` — all 64 intact and in order, bystander answered in **0.2ms**,
connection still in sync afterwards. Validated against the *pre-fix* build,
where it fails with "peer closed mid-reply" — the test genuinely catches the bug
it was written for rather than merely passing.

`panic_policy.py`: per-connection panic contained (worker did not restart);
top-level panic recovered in **2.5s** as a new process; a SQL session held open
across that restart **survived**, confirming the worker's status-1 FATAL exit
restarts only the worker. An external `kill -9` is different and *does* force
cluster-wide recovery — see `docs/ops.md`.
