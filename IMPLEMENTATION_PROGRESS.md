# Implementation progress

Each accepted step is reviewed, checked and committed independently, then pushed to `origin/main`. Provider support remains unverified until surface-specific live evidence exists.

| Step | Scope | Status | Evidence |
|---|---|---|---|
| 0 | Git bootstrap and implementation workflow | Accepted | Local Git initialized; design documents intentionally retained locally per user instruction |
| 1 / T02.1 | Rust workspace, SQLite migration and file-backed store | Accepted | 7 file-backed tests passed; fmt and clippy checks passed on Windows Rust 1.99.1 |
| 2 / T02.2 | Atomic event ingestion and restart-safe dedupe | Pending | No implementation claim |
| 3 / T02.3 | Usage reconciliation and session query | Pending | No implementation claim |
| 4 / T03 | Advisory policy | Pending | No runtime enforcement claim |
| 5 / T04–T05 | Codex observer and setup preview | Pending | VS Code and Desktop verified separately; synthetic fixtures are not live evidence |

Tooling is provisioned locally under ignored `.tools` where required. No provider configuration changes or paid model calls are part of these core steps.
