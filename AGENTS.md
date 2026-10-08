# Repository agent instructions

## Binding Git workflow

Read [GIT_WORKFLOW.md](GIT_WORKFLOW.md) before work, after compaction and before
publishing. On Andrew's workstation also read the original
`/Users/andrewmasiye/.codex/AGENTS.md`. Andrew's develop-to-main rule applies
only where he has created a `develop` branch. This repository has that existing
branch: NEVER commit, merge locally or push directly to `main`, including private
documentation. Changes integrate into `develop`; `main` receives only a PR from
this repository's `develop`.
Repositories found without Andrew-created `develop` are outside this remediation
and must be left alone; do not create that branch or infer direct-main permission.
Remote working branches, history rewrites and branch deletion require explicit
scoped authorization. Within the existing-develop scope, prior direct-main
exception claims are superseded by Andrew's 2026-10-08 rule.
Install the tracked Git guards before committing or publishing here.
