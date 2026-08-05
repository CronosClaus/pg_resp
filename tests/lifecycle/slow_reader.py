#!/usr/bin/env python3
"""ADD1: partial-write correctness under a slow reader.

The bug this guards against was live, not hypothetical: the server called
`write_all` on a **non-blocking** socket. `write_all` treats `WouldBlock` as a
hard error, so as soon as a reply did not fit in the socket buffer the rest of
the bytes were dropped and the client was left holding a truncated reply on a
desynchronized stream. Small replies on localhost always fit, which is why two
phases of testing never saw it — it takes a large reply plus a reader that does
not drain promptly, which is exactly what bible §10's 16KB-value, pipeline-16
benchmark arms will produce.

Two properties are asserted:

  1. **No corruption.** A client that pipelines many large GETs and reads
     slowly must receive every reply, whole and in order.
  2. **No head-of-line blocking.** While that slow client is being served, a
     second, ordinary client must keep getting prompt answers. A server that
     blocked or spun on the slow socket would fail this even if property 1 held.

Usage:
    python3 tests/lifecycle/slow_reader.py [--host 127.0.0.1] [--port 6379]
        [--password secret123] [--value-size 65536] [--pipeline 64]
"""

import argparse
import socket
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 2)[0] + "/differential")
from generate_and_compare import build_command, read_one_reply  # noqa: E402

FAILURES = []


def check(label, ok, detail=""):
    if ok:
        print(f"  ok    {label}")
    else:
        FAILURES.append(f"{label}{': ' + detail if detail else ''}")
        print(f"  FAIL  {label}{': ' + detail if detail else ''}")


def connect(host, port, password):
    sock = socket.create_connection((host, port), timeout=30)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    buf = b""
    if password:
        sock.sendall(build_command("AUTH", password))
        reply, buf = read_one_reply(sock, buf)
        if not reply.startswith(b"+"):
            raise RuntimeError(f"AUTH failed: {reply!r}")
    return sock, buf


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6379)
    ap.add_argument("--password", default=None)
    ap.add_argument("--value-size", type=int, default=64 * 1024)
    ap.add_argument("--pipeline", type=int, default=64)
    args = ap.parse_args()

    key = "slowreader:payload"
    value = ("x" * args.value_size).encode()
    expected_reply = b"$" + str(len(value)).encode() + b"\r\n" + value

    print(f"\nADD1 — partial writes under a slow reader "
          f"({args.pipeline} x {args.value_size}B pipelined)")

    # Seed the value with a normal client.
    setter, buf = connect(args.host, args.port, args.password)
    setter.sendall(build_command("SET", key, value))
    reply, buf = read_one_reply(setter, buf)
    check("seed SET succeeded", reply.startswith(b"+OK"), repr(reply[:40]))

    # A deliberately small receive buffer makes the server hit WouldBlock
    # quickly: the kernel cannot hand off more than the reader is willing to
    # take, so the reply tail must be buffered and flushed later.
    slow = socket.create_connection((args.host, args.port), timeout=30)
    slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
    slow.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    slow_buf = b""
    if args.password:
        slow.sendall(build_command("AUTH", args.password))
        reply, slow_buf = read_one_reply(slow, slow_buf)

    # Fire the whole pipeline at once, then read deliberately slowly.
    pipeline_bytes = b"".join(build_command("GET", key) for _ in range(args.pipeline))
    slow.sendall(pipeline_bytes)

    # While the server is holding buffered replies for `slow`, a bystander must
    # still be served promptly. Checked before draining the slow client, so the
    # server genuinely has pending writes outstanding at this moment.
    time.sleep(0.3)
    bystander, b_buf = connect(args.host, args.port, args.password)
    started = time.monotonic()
    bystander.sendall(build_command("PING"))
    reply, b_buf = read_one_reply(bystander, b_buf)
    elapsed = time.monotonic() - started
    check("a bystander connection is still answered", reply.startswith(b"+PONG"), repr(reply[:40]))
    check(f"bystander latency is not blocked by the slow reader ({elapsed*1000:.1f}ms)",
          elapsed < 2.0, f"{elapsed:.3f}s")

    # Now drain the slow client in small chunks and verify every reply arrives
    # intact and in order.
    slow.settimeout(30)
    received = 0
    corrupt = None
    for i in range(args.pipeline):
        try:
            reply, slow_buf = read_one_reply(slow, slow_buf)
        except Exception as exc:  # noqa: BLE001
            corrupt = f"reply {i} failed to parse: {exc}"
            break
        if reply != expected_reply:
            corrupt = (f"reply {i} differs: got {len(reply)} bytes, "
                       f"expected {len(expected_reply)}")
            break
        received += 1
        time.sleep(0.002)  # keep the reader slow throughout

    check(f"all {args.pipeline} large replies arrived intact",
          received == args.pipeline and corrupt is None,
          corrupt or f"only {received}/{args.pipeline}")

    # The connection must still be usable afterwards — a desynced stream would
    # show up here even if the byte counts happened to line up. Wrapped,
    # because the pre-fix failure mode is the peer *closing* here rather than
    # replying, and a stack trace is a worse test report than a FAIL line.
    try:
        slow.sendall(build_command("PING"))
        reply, slow_buf = read_one_reply(slow, slow_buf)
        still_synced, detail = reply.startswith(b"+PONG"), repr(reply[:40])
    except Exception as exc:  # noqa: BLE001
        still_synced, detail = False, f"{type(exc).__name__}: {exc}"
    check("the slow connection is still in sync afterwards", still_synced, detail)

    for sock in (setter, slow, bystander):
        sock.close()

    if FAILURES:
        print("\nFAILURES:")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    print("\nall slow-reader checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
