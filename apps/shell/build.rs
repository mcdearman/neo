//! Compiles the helper that finds and moves other apps' windows on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_windows.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_windows.m").flag("-fobjc-arc").compile("neo_mac_windows");
    for framework in ["AppKit", "ApplicationServices", "CoreGraphics", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
