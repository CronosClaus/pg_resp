#!/usr/bin/env python3
"""Proves generate_and_compare.py's own mechanics (RESP2 reply parsing,
command building, diffing) work, by pointing "candidate" and "oracle" at the
SAME pg_resp instance and DEL-ing every key the generated stream touches
before each side's run, so both start from an identical empty state.

This is NOT a differential test against Valkey (there is no second,
independent implementation involved) — a clean result here means "the
harness correctly replays and compares," not "pg_resp matches Valkey."
See generate_and_compare.py's module docstring and reports/phase1.md for
why the real Valkey comparison is PARTIAL(docker)/PARTIAL(no oracle binary).

Usage: python3 mechanics_selftest.py --port 6379
"""
import argparse
import socket
import sys

from generate_and_compare import build_command, random_t0_stream, run_stream
import random


def clear_keys(host, port, keys):
    sock = socket.create_connection((host, port), timeout=5)
    sock.sendall(build_command("DEL", *keys))
    sock.recv(65536)
    sock.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--commands", type=int, default=500)
    args = ap.parse_args()

    keys = [f"k{i}" for i in range(20)]
    rng = random.Random(args.seed)
    commands = random_t0_stream(rng, args.commands)

    clear_keys(args.host, args.port, keys)
    candidate = run_stream(args.host, args.port, commands)

    clear_keys(args.host, args.port, keys)
    oracle = run_stream(args.host, args.port, commands)

    mismatches = [
        (i, cmd, c, o)
        for i, (cmd, c, o) in enumerate(zip(commands, candidate, oracle))
        if c != o
    ]
    print(f"{len(commands)} commands replayed, {len(mismatches)} mismatches")
    for i, cmd, c, o in mismatches[:10]:
        print(f"  #{i} {cmd}: run1={c!r} run2={o!r}")

    if mismatches:
        print("HARNESS BUG: identical inputs against the identical server "
              "produced different outputs — fix the harness before trusting "
              "any real differential result.")
    else:
        print("Harness mechanics OK: deterministic replay against a cleared "
              "store produces identical responses both times.")
    sys.exit(1 if mismatches else 0)


if __name__ == "__main__":
    main()
