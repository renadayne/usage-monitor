# Agent Usage Monitor

## Current Implementation Status

The Rust workspace currently includes:
- Versioned SQLite migrations through `user_version = 3`, preserving older sessions and adding phase-bound inference contexts and independent model projection checkpoints.
- Connection configuration and verification (`foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL`, bounded `busy_timeout`).
- Safe refusal of unknown future `user_version` databases and non-empty unversioned databases prior to journal or schema mutation.
- A strict lifecycle/model-evidence DTO crate (`normalized-schema`). Unknown content, usage and quota fields are rejected in this stage. Served-model claims require direct response evidence, an inference ID and phase epoch.
- Bounded read-only session/timeline queries with keyset cursors. Timeline results exclude raw payload JSON and workspace paths. Storage time formatting uses fixed-width UTC nanoseconds for stable ordering.
- Lifecycle ingestion for `session.started`, `session.ended` and explicit local phase changes. Integration/account bindings are immutable. Event, snapshot, checkpoint and outbox writes commit together; retries ignore only receipt time and conflicting event content is rejected.

The independent `policy-engine` crate evaluates advisory rules with scope/priority precedence, tied-rule intersection and expiring session/phase-scoped overrides. Matching selection metadata stays indeterminate; only fresh served proof bound to the same provider/session/phase/epoch can establish compliance or violation. Requests to block are reported as enforcement unavailable. This pure engine has no provider-call control or persisted notification wiring yet.

The `adapters` crate contains a pure Codex hook parser with synthetic fixtures for separate VS Code and Desktop surface identifiers. It discards content/path fields, preserves common hook model metadata as selection evidence and rejects subagent hooks whose parent session cannot safely identify the child. It performs no file/network I/O or provider configuration changes. Parsed lifecycle/model metadata can be ingested; an integration test covers the synthetic parser-to-store path.

Model projection keeps selected, requested, resolved and served fields separate. Served snapshots require a matching durable inference start in the same source/session/phase/epoch. Old-phase, expired, unsequenced updates and competing sources remain historical when they cannot safely replace a head. A first observation can establish a head; replacing it requires increasing source sequence and nondecreasing observation time. Changing phase clears current model fields while retaining history. Turn/inference events are retained without guessing idle from completion or silence. Observer ingestion cannot change phase; the local user-action API is a separate entry point.

The pure `usage-accounting` crate reconciles a single cumulative stream per explicit epoch. Mid-counter attachment establishes a baseline, repeated/older revisions add nothing, and a newer decrease freezes that epoch until an explicit new epoch is selected. All values and arithmetic must stay finite and nonnegative. It does not select canonical series or persist usage yet; callers must retain checkpoints per complete integration/account/counter/scope/series/unit/metric/epoch identity and never add overlapping sources or parent/child aggregates.

Durable usage ingestion, desktop UI/Tauri shell, HTTP/OTLP endpoints and hook receivers are **not implemented yet**.

## Toolchain Prerequisites (Windows)

Building `rusqlite` with `bundled` SQLite on Windows requires a Rust stable toolchain (`2021` edition) and a C/C++ compiler + linker from either:
- **Visual Studio Build Tools** (Desktop development with C++ / MSVC toolchain), or
- **GNU toolchain** (MinGW-w64 / LLVM-MinGW with `x86_64-pc-windows-gnu`).

## Verification Commands

```powershell
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The current Windows build was checked with Rust 1.99.1 and LLVM-MinGW MSVCRT. Dependencies are locked in `Cargo.lock`. If the workspace-local tools have already been provisioned under ignored `.tools`, PowerShell can run:

```powershell
.\scripts\cargo-local.ps1 @('test', '--workspace', '--locked', '--offline')
.\scripts\cargo-local.ps1 @('fmt', '--all', '--', '--check')
.\scripts\cargo-local.ps1 @('clippy', '--workspace', '--all-targets', '--locked', '--offline', '--', '-D', 'warnings')
```

The helper configures only its process environment; it does not install tools or change the system PATH. Normal installed toolchains can use the `cargo` commands directly. Setup references: [Rust installation](https://rust-lang.org/tools/install/) and [rusqlite](https://docs.rs/rusqlite/0.37.0/rusqlite/).

Design documents and the original archive are intentionally excluded from the public repository. All current tests use synthetic local databases; no provider integration or live usage has been verified.

Local Windows Application Control can reject newly compiled build/test executables (`os error 4551`). The project does not modify or bypass that policy. When this occurs, the Windows/Linux GitHub CI results provide validation before the step is accepted on `main`.
