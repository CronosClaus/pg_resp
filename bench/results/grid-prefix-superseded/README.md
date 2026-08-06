# SUPERSEDED — pre-TCP_NODELAY-fix cells, retained as a record only

12 cells from the first Stage B attempt (all `P-def`, 64 B and 1 KB), stopped at
12/108. **These numbers must not appear in any published table.**

Superseded for two independent reasons:

1. They were measured against the pg_resp image built **before** the
   `TCP_NODELAY` fix on accepted sockets (fixed at commit `fbdcbbd`). The
   re-measured cells come from the post-fix image, and mixing the two would put
   figures from two different artifacts in one table.
2. The run they belong to was stopped mid-grid, so they cover one arm only and
   cannot support any comparison.

Retained rather than deleted because they are the evidence for two findings that
do get published in methodology: the 41 ms transport artefact that stopped the
run (ENV.md §22), and the `P-def` pipeline-16 spread instability (ENV.md §23).

The measured effect of the fix at this payload size was within noise, as
expected — replies at 1 KB are single-segment: `d1024-p16-c8` went 391,697 ->
398,154 ops/s, a +1.65% delta inside that cell's own 2.38% run-to-run spread.
That is a verification that the fix changed nothing here, not a performance
claim.
