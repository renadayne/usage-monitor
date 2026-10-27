fn main() {
    #[cfg(all(windows, feature = "desktop-shell"))]
    println!("cargo:rerun-if-changed=../dist");
    #[cfg(all(windows, feature = "desktop-shell"))]
    tauri_build::build();
}
