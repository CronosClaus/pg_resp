# NOT A FILEABLE FINDING — memtier 16 KB boundary did not reproduce off the bench box

**Status: recorded, NOT to be filed.** Written because a negative result that
nobody writes down gets rediscovered, and because the positive version of this
claim briefly made it into `ENV.md` in strong form.

## The claim that was going to be filed

On the Phase 4 benchmark box, `memtier_benchmark` at the pinned commit
(`272eeb647df5`) collapsed by three orders of magnitude at exactly a 16,384-byte
`--data-size` with `--pipeline=1`:

```
16,340 B  ->  21,476 ops/s
16,384 B  ->      24.5 ops/s   (40.7 ms per operation)
```

It reproduced against **four different servers** (pg_resp, Redis, Valkey, Redka),
was invariant in per-operation latency across a 64x change in connection count,
and `redis-benchmark` against the same server showed no such effect
(19,355 req/s at 16,384 B). That looked conclusive: a client-side defect.

## Why it is not filed

**It does not reproduce on a second machine.** A clean build of the same pinned
commit, in a fresh container, against a stock `redis:8.2-alpine` with no pg_resp
involved anywhere:

| `--data-size` | memtier ops/s |
|---|---|
| 16,000 | 8,132 |
| 16,340 | 8,560 |
| 16,383 | 3,152 |
| **16,384** | **7,225** |
| 16,385 | 6,748 |
| 20,000 | 6,472 |

No cliff. The 16,383 row is run-to-run variance on a short 300-request run, not a
boundary — the value *above* it is faster.

An upstream issue reporting a defect that the reporter cannot reproduce outside one
destroyed machine is not a useful issue. So this stays here.

## What is actually established, stated narrowly

- **On that box**, at that kernel and configuration, memtier collapsed at
  16,384 B and `redis-benchmark` did not. The client was implicated *in that
  environment*.
- **It is not established as a memtier defect in general**, and `ENV.md` §25's
  original phrasing ("the artifact is memtier's") is too strong. Corrected there.
- The two environments differ in more than one variable — kernel (6.8.0-117 vs a
  WSL2 kernel), virtualization (KVM vs WSL2), CPU, and default `tcp_wmem`
  (16,384 on the box, 262,144 here) — so no single cause is isolated.

## The one loose thread, recorded for honesty

Default `tcp_wmem` on the box was **16,384** — exactly where the cliff sat — and on
this machine it is **262,144**, where there is no cliff. That is suggestive.

But raising the box's `tcp_wmem` to 262,144 at runtime **did not move the cliff**,
which is what refuted the send-buffer explanation in the first place. Either the
runtime change did not take effect on the path that mattered, or the correlation is
a coincidence across two machines that differ in several ways. **The box was
destroyed before this could be settled**, and it cannot be settled without it.

## Checked: memtier does NOT set `SO_SNDBUF`, which makes this stranger, not clearer

The obvious explanation for "raising `tcp_wmem` did not move the cliff" would be
that memtier sets its own send-buffer size explicitly, making the kernel default
irrelevant. **It does not.** Every `setsockopt` call in the pinned tree:

```
shard_connection.cpp:410   SO_KEEPALIVE
shard_connection.cpp:421   SO_LINGER
shard_connection.cpp:424   TCP_NODELAY
statsd.cpp:186,187         SO_SNDTIMEO / SO_RCVTIMEO   (statsd socket, not the workload)
```

No `SO_SNDBUF`, no `SO_RCVBUF`. So the kernel default *should* have governed the
send buffer, and raising `net.ipv4.tcp_wmem`'s default from 16,384 to 262,144
*should* have changed behaviour. It did not.

That removes the tidy explanation and leaves the contradiction sharper than before:
the cliff sat exactly at the box's default send-buffer size, the client does not
override that size, and raising it changed nothing. One of those three facts is
being misread, and the machine that could say which is gone.

## What would settle it

A second machine with a default `tcp_wmem` of 16,384, running the boundary probe
(`bench/harness/box/boundary-probe.sh`, committed) before and after raising it. If
the cliff appears and then disappears, the send-buffer explanation is right and the
runtime change on the box failed for a separate reason worth understanding. If it
appears and persists, the client is implicated and this becomes fileable.

Until then: **two upstream findings, not three.**
