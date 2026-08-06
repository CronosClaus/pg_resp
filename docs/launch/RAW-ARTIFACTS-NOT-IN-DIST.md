# Raw benchmark artifacts are not in this distribution

`docs/BENCHMARKS.md` and `ENV.md` cite per-cell raw files — memtier output, cell
summaries, CPU samples, run logs. Those are **5.4 MB across 291 files** and are
deliberately excluded from the PGXN source distribution, which exists to build the
extension rather than to carry its measurement record.

They are **not deleted and not summarised away**. Every one is committed in the
public repository:

<https://github.com/CronosClaus/pg_resp/tree/v0.1.0/bench/results>

`ENV.md` itself — the methodology, the environment, the corrections and every
withdrawn conclusion — **is** included here, because it is the document the numbers
depend on.
