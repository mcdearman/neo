//! Compiles the helper that asks macOS for its desktop picture, its
//! sound devices and its Bluetooth, on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_system.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_system.m").flag("-fobjc-arc").compile("neo_mac_system");
    for framework in ["AppKit", "CoreAudio", "Foundation", "IOBluetooth"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
