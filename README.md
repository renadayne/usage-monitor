# Agent Usage Monitor

## Current Implementation Status (Step T02.1)

Only the Rust workspace and durable SQLite store bootstrap (`crates/collector`) are implemented in this step:
- Initial 10-table SQLite schema (`crates/collector/migrations/0001_initial.sql`) managed via `PRAGMA user_version = 1`.
- Connection configuration and verification (`foreign_keys = ON`, `journal_mode = WAL`, `synchronous = FULL`, bounded `busy_timeout`).
- Safe refusal of unknown future `user_version` databases and non-empty unversioned databases prior to journal or schema mutation.

Desktop UI/Tauri shell, HTTP/OTLP ingestion endpoints, hook receivers, policy evaluation, and provider adapters are **not implemented yet**.

## Toolchain Prerequisites (Windows)

Building `rusqlite` with `bundled` SQLite on Windows requires a Rust stable toolchain (`2021` edition) and a C/C++ compiler + linker from either:
- **Visual Studio Build Tools** (Desktop development with C++ / MSVC toolchain), or
- **GNU toolchain** (MinGW-w64 / LLVM-MinGW with `x86_64-pc-windows-gnu`).

## Verification Commands

```powershell
cargo test -p collector
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
