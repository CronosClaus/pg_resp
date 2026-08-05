# Security policy

## Reporting a vulnerability

Email **ccclaudiucarare@gmail.com**. Please do not open a public issue for a
suspected vulnerability.

Include what you did, what happened, and the PostgreSQL and pg_resp versions.
A proof-of-concept is welcome but not required. You will get an
acknowledgement; if you do not hear back within a week, assume the mail was
lost and send it again.

pg_resp is a single-maintainer project, so there is no security team and no
guaranteed response window. That is stated here rather than implied by
silence, so you can weigh it before depending on the extension.

## Supported versions

| version | supported |
|---|---|
| 0.1.x | yes |
| < 0.1 | no — pre-release, never tagged |

PostgreSQL 16, 17 and 18 are supported and CI-tested on every commit
(current major plus two back). Older majors are not tested and not supported.

## What an operator is trusting

These are properties of the design, not open bugs. They are listed because a
reader deciding whether to install pg_resp needs them before installing, not
after. The full treatment, including a blast-radius analysis, is in
[`docs/ops.md`](docs/ops.md).

**Installation requires superuser, and the extension is not trusted.**
`pg_resp` loads via `shared_preload_libraries`, so installing it is a
server-level act requiring a restart, and `CREATE EXTENSION pg_resp` cannot be
performed by a non-superuser database owner. If you cannot grant that, you
cannot run pg_resp.

**The cache is a single cluster-wide namespace with no per-role isolation.**
Any role that can call `resp.get` can read anything any other role has cached,
and any client that can reach the RESP port can read the whole keyspace. There
is no per-key or per-role access control, and none is planned for 0.1. For this
reason `resp.*` is revoked from `PUBLIC` at install time (decision D12) and
granting it is a deliberate, documented act.

**The RESP port is a second network surface on your database host.** It
defaults to `pg_resp.bind_address = 127.0.0.1`, which is the safe default and
the one to keep. Widening it exposes the cache to whoever can reach that
address. `pg_resp.password` provides Redis-style AUTH; there is **no TLS in
0.1**, so a widened bind address sends the password and every cached value in
cleartext. If you need pg_resp reachable off-box, terminate TLS in front of it
and treat the password as a speed bump, not a boundary.

**The cache is ephemeral and never persisted** (decision D5). A PostgreSQL
restart, a crash, or a bgworker restart empties it. This is deliberate: it is a
cache. Do not store anything in it that is not reconstructible from the
database.

**Cached values are not encrypted at rest in memory**, and the bgworker's heap
is readable by anything that can read the postmaster's process memory — the
same trust boundary as PostgreSQL's own shared buffers.

## Reference-source hygiene

pg_resp's behavioural reference is Valkey (BSD-3) and the public RESP
specification. Redis source is never opened or consulted (decision D8). This is
a licensing constraint that protects the PostgreSQL License grant in
[`LICENSE`](LICENSE), and it applies to contributors too — see
[`CONTRIBUTING.md`](CONTRIBUTING.md).
