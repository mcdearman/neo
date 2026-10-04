//! Compiles the Objective-C media helper on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_media.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_media.m").flag("-fobjc-arc").flag("-Wno-deprecated-declarations").compile("neo_mac_media");
    for framework in ["AVFoundation", "CoreMedia", "CoreGraphics", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
