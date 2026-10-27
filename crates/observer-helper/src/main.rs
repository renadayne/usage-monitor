//! Non-blocking Codex observer helper binary (`aum-observer`).
//!
//! Waits at most 200ms from `main` entry (budget excludes OS process startup),
//! always writes and flushes `{}` to stdout, and exits with status 0 via
//! `std::process::exit(0)` so slow stdin or file/socket operations cannot keep
//! the hook process alive.

use std::ffi::OsString;
use std::io::Write;
use std::sync::mpsc;
use std::time::Instant;

use observer_helper::{parse_cli_args, run_worker, DEFAULT_MAIN_BUDGET};

fn main() {
    // 200ms execution budget measured from `main` entry (excludes OS process startup).
    let deadline = Instant::now() + DEFAULT_MAIN_BUDGET;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (tx, rx) = mpsc::sync_channel(1);

    let _ = std::thread::Builder::new()
        .name("aum-hook-delivery".into())
        .spawn(move || {
            let outcome = parse_cli_args(args).and_then(|config_path| {
                let stdin = std::io::stdin();
                run_worker(&config_path, stdin, deadline)
            });
            let _ = tx.send(outcome);
        });

    let remaining = deadline.saturating_duration_since(Instant::now());
    let _ = rx.recv_timeout(remaining);

    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(b"{}");
    let _ = stdout.flush();
    drop(stdout);
    std::process::exit(0);
}
