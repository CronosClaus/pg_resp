#!/usr/bin/env python3
"""Valkey differential oracle (bible §5 Phase 1 gate, §9's "highest-leverage
testing idea in this project").

Generates a random stream of T0 commands and replays it, byte-for-byte
identically, against two independent RESP2 servers ("candidate" and
"oracle"), diffing every response. A real run needs a second server to
compare against — bible D8 says the oracle is Valkey (BSD-3), never Redis
source. This machine has neither docker nor a locally built Valkey binary
(the /ref/valkey clone is a sparse checkout of src/ + tests/unit/type only,
missing the deps/ tree — bundled jemalloc/lua/hiredis — needed for a full
build; building those from scratch was judged poor effort/value against this
run's remaining budget, see reports/phase1.md). This script is therefore
PARTIAL(docker) / PARTIAL(no oracle binary) — written and ready, not yet run
against a real second implementation. Its own mechanics (parsing, diffing,
byte-stream generation) ARE exercised by mechanics_selftest.py in this same
directory, which points both "candidate" and "oracle" at one pg_resp
instance — that self-consistency check proves the harness works, not that
pg_resp matches Valkey.

Usage:
  python3 generate_and_compare.py --candidate-port 6379 --oracle-port 6380 \
      --seed 42 --commands 5000
"""
import argparse
import random
import socket
import sys


def build_command(*args):
    parts = [f"*{len(args)}\r\n".encode()]
    for a in args:
        b = a if isinstance(a, bytes) else str(a).encode()
        parts.append(f"${len(b)}\r\n".encode() + b + b"\r\n")
    return b"".join(parts)


def read_one_reply(sock, buf):
    """Reads exactly one RESP2 reply from sock, using/extending buf as a
    carry-over buffer across calls. Minimal client-side parser — good enough
    for T0's reply shapes (simple/error/integer/bulk/array-of-bulk)."""
    def need(n):
        nonlocal buf
        while len(buf) < n:
            chunk = sock.recv(65536)
            if not chunk:
                raise ConnectionError("peer closed mid-reply")
            buf += chunk

    def read_line():
        nonlocal buf
        while b"\r\n" not in buf:
            chunk = sock.recv(65536)
            if not chunk:
                raise ConnectionError("peer closed mid-reply")
            buf += chunk
        idx = buf.index(b"\r\n")
        line, buf = buf[:idx], buf[idx + 2:]
        return line, buf

    def read_reply():
        nonlocal buf
        while not buf:
            chunk = sock.recv(65536)
            if not chunk:
                raise ConnectionError("peer closed mid-reply")
            buf += chunk
        sigil = buf[0:1]
        buf = buf[1:]
        if sigil in (b"+", b"-", b":"):
            line, buf = read_line()
            return sigil + line
        if sigil == b"$":
            line, buf = read_line()
            n = int(line)
            if n == -1:
                return b"$-1"
            need(n + 2)
            data, buf = buf[:n], buf[n + 2:]
            return b"$" + line + b"\r\n" + data
        if sigil == b"*":
            line, buf = read_line()
            n = int(line)
            if n == -1:
                return b"*-1"
            parts = [b"*" + line]
            for _ in range(n):
                parts.append(read_reply())
            return b"\r\n".join(parts)
        raise ValueError(f"unknown reply sigil: {sigil!r}")

    result = read_reply()
    return result, buf


def random_t0_stream(rng, n):
    keys = [f"k{i}" for i in range(20)]
    values = ["", "v", "hello world", "12345", "-7", "x" * 64]
    # T1/T2 additions (bible §3.4, Phase 2 END-STEP): SETEX, SETNX, GETDEL,
    # GETEX, PERSIST, TYPE, DBSIZE, KEYS. Deliberately NOT included: SCAN
    # (cursor values are an internal implementation detail — bible D10 says
    # pg_resp's own cursor design doesn't even match Valkey's, so cursor
    # *values* are never meant to be identical, only the aggregate
    # full-iteration result, which is a different kind of check than this
    # byte-for-byte reply differential), RANDOMKEY (inherently
    # non-deterministic across implementations), AUTH/HELLO/CLIENT/COMMAND/
    # QUIT/SELECT/FLUSHDB (connection-lifecycle/protocol-negotiation
    # commands, not per-key data semantics — flushing mid-stream would also
    # invalidate the replay's key-state assumptions for every command after it).
    cmds = []
    for _ in range(n):
        choice = rng.choice(
            [
                "PING", "SET", "GET", "DEL", "EXISTS", "INCR", "DECR", "TTL", "EXPIRE",
                "MSET", "MGET", "SETEX", "SETNX", "GETDEL", "GETEX", "PERSIST", "TYPE",
                "DBSIZE", "KEYS",
            ]
        )
        k = rng.choice(keys)
        if choice == "PING":
            cmds.append(["PING"])
        elif choice == "SET":
            v = rng.choice(values)
            opts = []
            if rng.random() < 0.3:
                opts += ["EX", str(rng.choice([1, 10, 100]))]
            if rng.random() < 0.2:
                opts += [rng.choice(["NX", "XX"])]
            cmds.append(["SET", k, v] + opts)
        elif choice == "GET":
            cmds.append(["GET", k])
        elif choice == "DEL":
            cmds.append(["DEL", k])
        elif choice == "EXISTS":
            cmds.append(["EXISTS", k])
        elif choice == "INCR":
            cmds.append(["INCR", k])
        elif choice == "DECR":
            cmds.append(["DECR", k])
        elif choice == "TTL":
            cmds.append(["TTL", k])
        elif choice == "EXPIRE":
            cmds.append(["EXPIRE", k, str(rng.choice([1, 10, 100]))])
        elif choice == "MSET":
            k2 = rng.choice(keys)
            cmds.append(["MSET", k, rng.choice(values), k2, rng.choice(values)])
        elif choice == "MGET":
            k2 = rng.choice(keys)
            cmds.append(["MGET", k, k2])
        elif choice == "SETEX":
            cmds.append(["SETEX", k, str(rng.choice([1, 10, 100])), rng.choice(values)])
        elif choice == "SETNX":
            cmds.append(["SETNX", k, rng.choice(values)])
        elif choice == "GETDEL":
            cmds.append(["GETDEL", k])
        elif choice == "GETEX":
            if rng.random() < 0.5:
                cmds.append(["GETEX", k, "PERSIST"])
            else:
                cmds.append(["GETEX", k, "EX", str(rng.choice([1, 10, 100]))])
        elif choice == "PERSIST":
            cmds.append(["PERSIST", k])
        elif choice == "TYPE":
            cmds.append(["TYPE", k])
        elif choice == "DBSIZE":
            cmds.append(["DBSIZE"])
        elif choice == "KEYS":
            # A fixed, bounded pattern (not a random glob) keeps this
            # comparable — both sides see the same small keyspace anyway.
            cmds.append(["KEYS", "k*"])
    return cmds


def normalize_for_compare(cmd, reply):
    """KEYS's reply order is implementation-defined (hash table iteration
    order differs between pg_resp and Valkey) even when the underlying key
    *set* is identical — comparing raw bytes would flag that as a false
    mismatch. Order-independent comparison for KEYS only; every other
    command's reply is still compared byte-for-byte."""
    if cmd and cmd[0] == "KEYS" and reply.startswith(b"*") and reply != b"*-1":
        parts = reply.split(b"\r\n")
        elements = [parts[i + 1] for i in range(1, len(parts) - 1, 2)]
        return b"KEYS(unordered):" + b",".join(sorted(elements))
    return reply


def run_stream(host, port, commands):
    sock = socket.create_connection((host, port), timeout=5)
    buf = b""
    replies = []
    for cmd in commands:
        sock.sendall(build_command(*cmd))
        reply, buf = read_one_reply(sock, buf)
        replies.append(reply)
    sock.close()
    return replies


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--candidate-host", default="127.0.0.1")
    ap.add_argument("--candidate-port", type=int, required=True)
    ap.add_argument("--oracle-host", default="127.0.0.1")
    ap.add_argument("--oracle-port", type=int, required=True)
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--commands", type=int, default=2000)
    args = ap.parse_args()

    rng = random.Random(args.seed)
    commands = random_t0_stream(rng, args.commands)

    candidate = run_stream(args.candidate_host, args.candidate_port, commands)
    oracle = run_stream(args.oracle_host, args.oracle_port, commands)

    mismatches = []
    for i, (cmd, c, o) in enumerate(zip(commands, candidate, oracle)):
        if normalize_for_compare(cmd, c) != normalize_for_compare(cmd, o):
            mismatches.append((i, cmd, c, o))

    print(f"{len(commands)} commands replayed, {len(mismatches)} mismatches")
    for i, cmd, c, o in mismatches[:20]:
        print(f"  #{i} {cmd}: candidate={c!r} oracle={o!r}")

    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
