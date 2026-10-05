//! Compiles the ImageIO decoder on macOS, for formats such as HEIC.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_decode.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_decode.m").flag("-fobjc-arc").compile("neo_mac_decode");
    for framework in ["ImageIO", "CoreGraphics", "CoreFoundation", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
