#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(all(windows, feature = "desktop-shell"))]
mod shell;

#[cfg(all(windows, feature = "desktop-shell"))]
fn main() {
    shell::run();
}

#[cfg(not(all(windows, feature = "desktop-shell")))]
fn main() {
    eprintln!("Build the Windows desktop shell with --features desktop-shell. The core service is portable.");
}
