# Agent Usage Monitor

## Current Implementation Status

The Rust workspace currently includes:
- Versioned SQLite migrations through `user_version = 4`, preserving older sessions and adding phase-bound inference contexts, model checkpoints and immutable usage evidence.
- Connection configuration and verification (`foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL`, bounded `busy_timeout`).
- Safe refusal of unknown future `user_version` databases and non-empty unversioned databases prior to journal or schema mutation.
- A strict metadata DTO crate (`normalized-schema`) with a separate typed `UsageReport` envelope. Arbitrary content and quota fields are rejected. Served-model claims require direct response evidence, an inference ID and phase epoch.
- Bounded read-only session/timeline queries with keyset cursors. Timeline results exclude raw payload JSON and workspace paths. Storage time formatting uses fixed-width UTC nanoseconds for stable ordering.
- Lifecycle ingestion for `session.started`, `session.ended` and explicit local phase changes. Integration/account bindings are immutable. Event, snapshot, checkpoint and outbox writes commit together; retries ignore only receipt time and conflicting event content is rejected.

The independent `policy-engine` crate evaluates advisory rules with scope/priority precedence, tied-rule intersection and expiring session/phase-scoped overrides. Matching selection metadata stays indeterminate; only fresh served proof bound to the same provider/session/phase/epoch can establish compliance or violation. Requests to block are reported as enforcement unavailable. This pure engine has no provider-call control or persisted notification wiring yet.

The collector stores up to 500 local observe/notify policies, validates exact model IDs and rejects unavailable blocking actions. Read-only session policy dry runs use committed projection proof, current phase and explicit local clock trust; arbitrary later history cannot replace proof. Desktop-facing metadata views exclude provider config and payload JSON. Explicit 7/30-day CSV export uses observation times, excludes account identities/raw payloads and neutralizes spreadsheet formulas. Exports are capped at 10,000 rows and report truncation.

The `adapters` crate contains a pure Codex hook parser with synthetic fixtures for separate VS Code and Desktop surface identifiers. It discards content/path fields, preserves common hook model metadata as selection evidence and rejects subagent hooks whose parent session cannot safely identify the child. It performs no file/network I/O or provider configuration changes. Parsed lifecycle/model metadata can be ingested; an integration test covers the synthetic parser-to-store path.

Model projection keeps selected, requested, resolved and served fields separate. Served snapshots require a matching durable inference start in the same source/session/phase/epoch. Old-phase, expired, unsequenced updates and competing sources remain historical when they cannot safely replace a head. A first observation can establish a head; replacing it requires increasing source sequence and nondecreasing observation time. Changing phase clears current model fields while retaining history. Turn/inference events are retained without guessing idle from completion or silence. Observer ingestion cannot change phase; the local user-action API is a separate entry point.

The pure `usage-accounting` crate reconciles a single cumulative stream per explicit epoch. Mid-counter attachment establishes a baseline, repeated/older revisions add nothing, and a newer decrease freezes that epoch until an explicit new epoch is selected. All values and arithmetic must stay finite and nonnegative.

The collector commits usage reports, sample dedupe, checkpoints, accepted contributions and outbox markers together. Local configuration must select one canonical source/series/scope before usage arrives; observer reports cannot change it. Other sources and parent aggregates remain reconciliation evidence. Only direct quantitative evidence contributes; unverified samples cannot poison direct counter order or identity. Session totals remain separate by unit, metric and relationship to totals. Unknown-order snapshots and partial/frozen coverage stay explicit. Source switches, counter mode changes and changed breakdown semantics require reconciliation. There is no price lookup or conversion of credits, percentages or requests into tokens/USD. Time filters describe observed contributions, not provider billing windows.

The `setup-plan` crate creates an in-memory Codex `hooks.json` merge and a manifest of its exact added groups, then previews a three-way revert that preserves unrelated user edits and reports modified/missing/duplicated group conflicts. Input sizes and helper commands are bounded; ambiguous duplicate JSON keys are rejected. Redacted previews contain event names/counts only. Full config stays in memory and is not logged or saved by this crate. Empty event arrays may remain after revert. Existing matching commands are preserved without claiming ownership or coverage. The future apply layer must protect manifest integrity, verify the current file hash and obtain consent for the concrete diff. Non-managed Codex hooks require review/trust in the client; the planner does not grant trust. Schema reference: [Codex hooks](https://learn.chatgpt.com/docs/hooks).

The `observer-runtime` library binds only `127.0.0.1`, authenticates each Codex integration, rejects browser origins/foreign hosts and limits metadata bodies to 64 KiB. A bounded writer queue returns 429 when full and 503 when a commit fails or remains unconfirmed. A success response is produced only after durable ingestion commits. The endpoint accepts only the Codex parser's selection/lifecycle contract, with no served-model, phase or activity authority. Its listener/writer have explicit shutdown ownership. This is a library entry point; the desktop launcher and helper are separate steps. API references: [Axum](https://docs.rs/axum/0.8.9/axum/) and [Tokio channels](https://tokio.rs/tokio/tutorial/channels).

The `aum-observer` executable reads only the explicitly supplied app-owned config and bounded hook stdin, normalizes before transport, and sends the same event/body on at most two loopback attempts. Its main thread waits up to 200 ms from entry, then emits `{}` and exits successfully even when the collector/config/stdin is unavailable. OS process startup is outside that budget. Network attempts share the deadline; payloads are not spooled to disk. Events can be lost while the app is unavailable, so the helper cannot establish target verification or continuous coverage by itself. Tokens live in the app-owned config, not hook command arguments. Build with `cargo build -p observer-helper --release`; invocation is `aum-observer --config <app-owned-config.json>`. Do not point it at provider credential files. Synthetic tests include helper-to-receiver-to-SQLite transport.

The React desktop frontend has a dashboard, session timeline/phase control, policy editor, history export controls and a Codex setup preview screen. Browser mode starts without telemetry; synthetic demo data requires an explicit choice and remains separate from native state. The native IPC bridge is defined, while the Tauri shell is the next integration step. No provider connection is inferred from receiver readiness.

Tauri shell integration, quota ingestion, OTLP endpoints and actual provider setup writes are **not implemented yet**.

## Frontend preview

With Node 24.14.0 and npm installed:

```powershell
cd apps/desktop
npm ci --ignore-scripts
npm run test
npm run build
npm run dev
```

Open `http://127.0.0.1:1420` and explicitly choose the demo to preview synthetic data. Browser mode cannot operate native data or apply provider configuration. The tests check evidence/freshness classification, demo isolation and local phase IPC arguments. Desktop and mobile browser interactions have also been checked; native Windows lifecycle verification is separate.

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
