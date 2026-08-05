#!/usr/bin/env python3
"""Build the D14 curve tables from committed sweep.py cell artifacts.

D14 as amended (reports/phase4-progress.md §A.1) requires three things that a
single headline number cannot carry:

  1. the FULL throughput-vs-p99 curve for BOTH arms, not only the matched point;
  2. identical memtier client configuration on both arms of every compared cell;
  3. the secondary each-at-own-saturation ratio reported alongside the
     matched-p99 headline.

This reads the per-cell `.json` files sweep.py already writes, so the tables are
derived from committed artifacts rather than retyped from a terminal — the table
and the raw file cannot drift apart, and anyone can regenerate it.

Requirement 2 is ENFORCED, not assumed: a workload present for one arm and
absent for the other is reported as an unpaired cell and excluded from every
ratio. Comparing two arms at different client configurations is the easiest way
to produce a large, meaningless number.

Usage:
  python3 bench/harness/curve.py bench/results/stage-a
  python3 bench/harness/curve.py bench/results/stage-a --arms P-opt K-pg
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

# p99 ceilings for the matched-latency comparison. A cache is chosen under a
# latency budget, so "best throughput while staying under T" is the question an
# operator actually has. Several T values are reported because picking one would
# be picking the flattering one.
P99_TARGETS_MS = [1.0, 2.0, 5.0, 10.0, 25.0, 50.0]


def load_cells(d: pathlib.Path) -> list[dict]:
    cells = []
    for p in sorted(d.glob("*.json")):
        try:
            cells.append(json.loads(p.read_text()))
        except json.JSONDecodeError as e:
            print(f"warning: skipping unreadable {p.name}: {e}", file=sys.stderr)
    return cells


def fmt_pub(c: dict) -> str:
    if c.get("publishable"):
        return "yes"
    sp = c.get("spread_pct")
    if c.get("env_class") != "dedicated":
        return "NO (not official box)"
    if sp is None:
        return "NO (single run)"
    return f"NO (spread {sp:.2f}%)"


def curve_table(arm: str, cells: list[dict]) -> str:
    rows = sorted(
        [c for c in cells if c["arm"] == arm],
        key=lambda c: (c.get("workload_id", "")),
    )
    out = [
        f"### {arm} — throughput vs p99",
        "",
        "| workload | conns | ops/s | p50 ms | p99 ms | p99.9 ms | hit % | spread % | publishable |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for c in rows:
        sp = c.get("spread_pct")
        out.append(
            f"| {c['workload_id']} | {c['total_conns']} | {c['ops_sec']:,.0f} | "
            f"{c['p50']:.3f} | {c['p99']:.3f} | {c['p999']:.3f} | "
            f"{c['hit_rate_pct']:.2f} | {('%.2f' % sp) if sp is not None else '—'} | "
            f"{fmt_pub(c)} |"
        )
    return "\n".join(out)


def matched_p99(a: str, b: str, cells: list[dict], paired: set[str]) -> str:
    """Best publishable throughput each arm reaches while holding p99 <= T."""

    def best_under(arm: str, t: float):
        cand = [
            c
            for c in cells
            if c["arm"] == arm
            and c["workload_id"] in paired
            and c.get("publishable")
            and c["p99"] <= t
        ]
        return max(cand, key=lambda c: c["ops_sec"]) if cand else None

    out = [
        f"### Matched-p99 comparison — {a} vs {b}",
        "",
        "Best throughput each arm reaches while holding p99 at or below the "
        "budget, over paired cells only.",
        "",
        f"| p99 budget | {a} ops/s | at | {b} ops/s | at | ratio {a}/{b} |",
        "|---|---|---|---|---|---|",
    ]
    for t in P99_TARGETS_MS:
        ca, cb = best_under(a, t), best_under(b, t)
        if ca is None and cb is None:
            continue
        ra = f"{ca['ops_sec']:,.0f}" if ca else "—"
        rb = f"{cb['ops_sec']:,.0f}" if cb else "—"
        wa = ca["workload_id"] if ca else "—"
        wb = cb["workload_id"] if cb else "—"
        if ca and cb and cb["ops_sec"] > 0:
            ratio = f"**{ca['ops_sec'] / cb['ops_sec']:.2f}x**"
        elif ca and not cb:
            ratio = f"{b} cannot meet this budget"
        else:
            ratio = "—"
        out.append(f"| <= {t:g} ms | {ra} | {wa} | {rb} | {wb} | {ratio} |")
    return "\n".join(out)


def own_saturation(a: str, b: str, cells: list[dict]) -> str:
    """Each arm at its own best throughput, latency unconstrained (D14's
    secondary figure). Deliberately reported next to the matched-p99 table
    because the two answer different questions and the unconstrained ratio
    always looks larger."""

    def peak(arm: str):
        cand = [c for c in cells if c["arm"] == arm and c.get("publishable")]
        return max(cand, key=lambda c: c["ops_sec"]) if cand else None

    pa, pb = peak(a), peak(b)
    out = [
        f"### Each-at-own-saturation — {a} vs {b} (secondary)",
        "",
        "| arm | peak ops/s | at | p99 there | hit % |",
        "|---|---|---|---|---|",
    ]
    for arm, c in ((a, pa), (b, pb)):
        if c:
            out.append(
                f"| {arm} | {c['ops_sec']:,.0f} | {c['workload_id']} "
                f"({c['total_conns']} conns) | {c['p99']:.3f} ms | {c['hit_rate_pct']:.2f} |"
            )
        else:
            out.append(f"| {arm} | no publishable cell | — | — | — |")
    if pa and pb and pb["ops_sec"] > 0:
        out += [
            "",
            f"Unconstrained ratio: **{pa['ops_sec'] / pb['ops_sec']:.2f}x** — note the "
            "two arms are at DIFFERENT latencies here "
            f"(p99 {pa['p99']:.3f} ms vs {pb['p99']:.3f} ms), which is exactly why "
            "the matched-p99 table above is the headline and this one is secondary.",
        ]
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("results_dir")
    ap.add_argument("--arms", nargs=2, default=["P-opt", "K-pg"])
    args = ap.parse_args()

    d = pathlib.Path(args.results_dir)
    cells = load_cells(d)
    if not cells:
        print(f"no cell artifacts in {d}", file=sys.stderr)
        return 1

    a, b = args.arms
    wa = {c["workload_id"] for c in cells if c["arm"] == a}
    wb = {c["workload_id"] for c in cells if c["arm"] == b}
    paired = wa & wb
    unpaired = (wa | wb) - paired

    print(f"# D14 curve tables — {a} vs {b}")
    print()
    print(f"Source: `{d}` — {len(cells)} cell artifacts, "
          f"{sum(1 for c in cells if c.get('publishable'))} publishable.")
    print()
    if unpaired:
        print("**Unpaired cells excluded from every ratio** (D14 requires identical "
              "client configuration on both arms of a compared cell): "
              + ", ".join(sorted(unpaired)))
        print()
    print(curve_table(a, cells))
    print()
    print(curve_table(b, cells))
    print()
    print(matched_p99(a, b, cells, paired))
    print()
    print(own_saturation(a, b, cells))
    return 0


if __name__ == "__main__":
    sys.exit(main())
