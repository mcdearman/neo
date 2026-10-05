//! Compiles the app-icon reader on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_icon.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_icon.m").flag("-fobjc-arc").compile("neo_mac_icon");
    for framework in ["AppKit", "CoreGraphics", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
