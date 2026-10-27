# Implementation progress

Each accepted step is reviewed, checked and committed independently, then pushed to `origin/main`. Provider support remains unverified until surface-specific live evidence exists.

| Step | Scope | Status | Evidence |
|---|---|---|---|
| 0 | Git bootstrap and implementation workflow | Accepted | Local Git initialized; design documents intentionally retained locally per user instruction |
| 1 / T02.1 | Rust workspace, SQLite migration and file-backed store | Accepted | 7 file-backed tests passed; fmt and clippy checks passed on Windows Rust 1.99.1 |
| 1a | Windows/Linux GitHub CI | Accepted | Both platforms passed tests, formatting and clippy for the store bootstrap |
| 2a / T02.2a | Strict lifecycle schema and read-only paginated queries | Accepted | Windows/Linux CI passed; merged via PR #1 |
| 2b / T02.2b | Atomic lifecycle ingestion and restart-safe dedupe | Accepted | Windows/Linux CI passed; migration v2, retry/namespace/rollback/phase authority tests merged via PR #2 |
| 3a / T02.3 | Pure cumulative usage reconciliation | Accepted | Windows/Linux CI passed; merged via PR #6; explicit epochs, baselines, replay/late revisions and decrease freeze |
| 3b / T02.3 | Durable usage ingestion and scoped totals | Accepted | Windows/Linux CI passed; merged via PR #7; canonical-series selection, trust isolation, non-addition and atomic rollback/restart tests |
| 4 / T03 | Pure advisory policy engine | Accepted | Windows/Linux tests, fmt and clippy passed; merged via PR #3; no runtime enforcement |
| 5a / T04 | Pure Codex hook parser | Accepted | Windows/Linux CI passed; merged via PR #4; separate VS Code/Desktop identifiers and privacy fixtures, no live verification |
| 2c / T02.2c | Phase-bound inference/model projection | Accepted | Windows/Linux CI passed; merged via PR #5; durable start binding, late-phase/history handling, restart/rollback and Codex-to-store synthetic tests |
| 5b / T05 | In-memory Codex setup/revert preview | Accepted | Windows/Linux CI passed; merged via PR #8; no provider config writes or trust changes |
| 5c / T04 | Authenticated Codex loopback intake | Accepted | Windows/Linux CI passed; merged via PR #9; bounded queue/body, ACK-after-commit, auth/origin and explicit shutdown tests |
| 5d / T04 | Bounded fail-open Codex hook helper | Review/CI pending | Stable retry ID/body, metadata filtering before transport, open-stdin deadline test and synthetic helper-to-store roundtrip; no live verification |

Tooling is provisioned locally under ignored `.tools` where required. No provider configuration changes or paid model calls are part of these core steps.
