fn main() {
    #[cfg(all(windows, feature = "desktop-shell"))]
    tauri_build::build();
}
