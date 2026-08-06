#!/usr/bin/env python3
"""Golden test for curve.py.

Run: python3 -m unittest discover -s bench/harness -p 'test_*.py' -v

WHY THIS TEST EXISTS
--------------------
curve.py generates the tables that become published claims, and it has already
produced one wrong claim: the first matched-p99 implementation reported **312x**
by comparing pg_resp at 64 B against Redka at 1 KB. Nothing was wrong with the
measurements; the generator picked each arm's best cell independently and paired
two different workloads. The error ran in the flattering direction, which is the
direction this project is required to be most suspicious of (bible §0.5).

So the fix is pinned by a test rather than by having been careful once. The
central assertion is STRUCTURAL: inside any single matched-p99 payload block,
every workload referenced must belong to that payload size. Fixtures below are
built specifically so that a cross-payload pairing would be enormously flattering
if it were possible — if someone reintroduces the bug, this test fails loudly
instead of a 300x number reaching a README.
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

HARNESS = pathlib.Path(__file__).resolve().parent
CURVE = HARNESS / "curve.py"


def cell(
    arm, data_size, pipeline, conns, ops, p99,
    hit=50.0, publishable=True, spread=1.0, p50=None, p999=None,
):
    """One synthetic cell artifact, shaped like sweep.py's real output."""
    threads = 4 if conns >= 4 else 1
    clients = max(1, conns // threads)
    return {
        "arm": arm,
        "workload_id": f"d{data_size}-p{pipeline}-c{conns}",
        "env_class": "dedicated",
        "publishable": publishable,
        "total_conns": conns,
        "ops_sec": float(ops),
        "p50": float(p50 if p50 is not None else p99 / 2),
        "p99": float(p99),
        "p999": float(p999 if p999 is not None else p99 * 1.5),
        "avg_latency": float(p99) / 2,
        "hits_sec": ops * hit / 100.0,
        "misses_sec": ops * (100.0 - hit) / 100.0,
        "hit_rate_pct": float(hit),
        "spread_pct": spread,
        "n_runs": 3,
        "spread_ok": True,
        "raw_file": f"bench/results/x/{arm}.txt",
        "rerun": (
            f"bench/harness/sweep.py --arm {arm} --data-size {data_size} "
            f"--pipeline {pipeline} --clients {clients} --threads {threads} "
            f"--test-time 60 --run-count 3 --warmup-keys 200000 "
            f"--client-cpus 4-7 --client-pin-mechanism docker-cpuset"
        ),
        "data_size": data_size,
        "pipeline": pipeline,
        "clients": clients,
        "threads": threads,
        "warmup_protocol": "v2",
        "warmup_keys": 200000,
    }


def write_cells(d: pathlib.Path, cells: list[dict]) -> None:
    for i, c in enumerate(cells):
        (d / f"{i:03d}-{c['arm']}-{c['workload_id']}.json").write_text(json.dumps(c))


def run_curve(d: pathlib.Path, a="A-fast", b="B-slow") -> str:
    proc = subprocess.run(
        [sys.executable, str(CURVE), str(d), "--arms", a, b],
        capture_output=True, text=True,
    )
    assert proc.returncode == 0, f"curve.py failed: {proc.stderr}"
    return proc.stdout


# The trap fixture. A-fast is spectacular at 64 B and unremarkable at 1 KB;
# B-slow is the reverse. A cross-payload pairing would report A's 64 B peak
# (1,000,000) against B's worst 1 KB cell (1,000) = 1000x. The honest
# same-payload answers are 2.0x at 64 B and 5.0x at 1 KB.
TRAP_CELLS = [
    cell("A-fast", 64, 16, 32, 1_000_000, 0.5),
    cell("B-slow", 64, 16, 32, 500_000, 0.5),
    cell("A-fast", 1024, 16, 32, 5_000, 20.0),
    cell("B-slow", 1024, 16, 32, 1_000, 20.0),
]


class CrossPayloadIsStructurallyImpossible(unittest.TestCase):
    def test_matched_p99_never_pairs_different_payload_sizes(self):
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, TRAP_CELLS)
            out = run_curve(d)

        section = out.split("### Matched-p99 comparison")[1].split("### Each-at-own")[0]

        # Walk the payload sub-blocks and check every workload id named inside a
        # block belongs to that block's payload size. This is the invariant the
        # 312x bug violated.
        blocks = re.split(r"\*\*Payload (\d+) B\*\*", section)[1:]
        self.assertGreaterEqual(len(blocks), 4, "expected at least two payload blocks")
        checked = 0
        for size, body in zip(blocks[0::2], blocks[1::2]):
            for wid in re.findall(r"d(\d+)-p\d+-c\d+", body):
                self.assertEqual(
                    wid, size,
                    f"matched-p99 block for payload {size} B referenced a "
                    f"{wid} B workload — this is the 312x cross-payload bug",
                )
                checked += 1
        self.assertGreater(checked, 0, "no workloads referenced; test proved nothing")

    def test_the_flattering_cross_payload_ratio_never_appears(self):
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, TRAP_CELLS)
            out = run_curve(d)
        # 1,000,000 / 1,000 = 1000x would be the cross-payload artefact.
        self.assertNotIn("1000.00x", out)
        # The honest same-payload ratios must both be present.
        self.assertIn("2.00x", out)
        self.assertIn("5.00x", out)

    def test_own_saturation_is_also_per_payload(self):
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, TRAP_CELLS)
            out = run_curve(d)
        sat = out.split("### Each-at-own-saturation")[1]
        self.assertIn("Unconstrained ratio at 64 B", sat)
        self.assertIn("Unconstrained ratio at 1024 B", sat)


class HandVerifiedCell(unittest.TestCase):
    """One cell whose every published field is computed by hand here, so the
    test pins the arithmetic and not merely the formatting."""

    def test_single_cell_arithmetic(self):
        # A-fast: 1,000 ops/s. B-slow: 250 ops/s. Same workload, so the ratio is
        # exactly 1000/250 = 4. Hit rate 40% of 1,000 ops = 400 hits/s and 600
        # misses/s, so the reported hit rate must read 40.00.
        cells = [
            cell("A-fast", 1024, 1, 8, 1_000, 3.0, hit=40.0),
            cell("B-slow", 1024, 1, 8, 250, 9.0, hit=40.0),
        ]
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, cells)
            out = run_curve(d)

        self.assertIn("**4.0x**", out, "hand-computed 1000/250 = 4.0x not found")
        self.assertIn("| 1,000 |", out, "ops/s not rendered with thousands separator")
        self.assertIn("40.00", out, "hand-computed hit rate 40.00% not found")
        # p99 3.0 <= 5 ms budget for A; B's 9.0 exceeds 5 but meets 10.
        matched = out.split("### Matched-p99")[1].split("### Each-at-own")[0]
        self.assertIn("B-slow meets no cell at this budget", matched)


class PublishabilityIsEnforced(unittest.TestCase):
    def test_unpublishable_cells_never_enter_a_ratio(self):
        cells = [
            cell("A-fast", 1024, 16, 32, 900_000, 1.0, publishable=False, spread=19.0),
            cell("B-slow", 1024, 16, 32, 1_000, 1.0),
        ]
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, cells)
            out = run_curve(d)
        # 900x would be the ratio if the unpublishable cell were used.
        self.assertNotIn("900.00x", out)
        self.assertIn("NO (spread 19.00%)", out)

    def test_single_run_cell_is_stamped_not_silently_dropped(self):
        c = cell("A-fast", 1024, 16, 32, 100, 1.0, publishable=False)
        c["spread_pct"] = None
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, [c, cell("B-slow", 1024, 16, 32, 50, 1.0)])
            out = run_curve(d)
        self.assertIn("NO (single run)", out)


class ClientConfigLineComesFromTheRerunCommand(unittest.TestCase):
    def test_config_line_reflects_rerun_not_a_restatement(self):
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, [cell("A-fast", 64, 16, 32, 10, 1.0)])
            out = run_curve(d, "A-fast", "A-fast")
        self.assertIn("data-size=64", out)
        self.assertIn("pipeline=16", out)
        self.assertIn("warmup-keys=200000", out)
        self.assertIn("client-pin-mechanism=docker-cpuset", out)

    def test_missing_rerun_is_reported_not_invented(self):
        c = cell("A-fast", 64, 16, 32, 10, 1.0)
        del c["rerun"]
        with tempfile.TemporaryDirectory() as td:
            d = pathlib.Path(td)
            write_cells(d, [c])
            out = run_curve(d, "A-fast", "A-fast")
        self.assertIn("(no rerun command recorded)", out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
