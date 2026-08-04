---
name: pg-conventions
description: Postgres-community conventions for pg_resp's public surfaces — error message style, GUC naming, control file and versioned SQL scripts, docs tone, the contrib-quality reviewer checklist. Consult before writing any user-facing string, SQL object, doc page, or release artifact.
---
# PG conventions

**STATUS: STUB — Phase 0 task 3 fills this** from /ref/postgres (contrib/pg_stat_statements as the gold standard) and the PG error-style guide, cross-checked against /ref/pg_net's packaging.

Required contents when filled (bible §7 + §8):
1. Error message style distilled: primary message lowercase-no-period, errdetail/errhint usage, no exclamation marks, pgrx ereport equivalents.
2. GUC naming/documentation conventions; how contrib modules document GUCs.
3. Extension packaging: control file fields, versioned sql/pg_resp--X.Y.Z.sql rules, upgrade script discipline from v0.1.0 onward.
4. SQL object conventions: schema `resp`, function naming, comments on objects.
5. Docs structure mirroring a contrib module page: overview → configuration → functions → caveats.
6. The reviewer checklist (bible §8) as an actionable pre-release gate list.
