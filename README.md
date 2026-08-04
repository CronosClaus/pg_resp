# pg_resp — scaffold

Redis-protocol cache inside a Postgres background worker. Spec: [`project-bible.md`](project-bible.md). Method: [`docs/RUNBOOK.md`](docs/RUNBOOK.md).

Status: Phase 0 not started. This is the agent scaffold, not the extension.

## Start
```bash
claude                 # inside WSL2/Linux, logged in
/model                 # verify: sonnet→Sonnet 5, opus→Opus 5
/kickoff 0             # plans on Opus, waits for your approval
/model sonnet          # then execute
```
Close every phase with `/phase-report N`, commit, `/clear`.
