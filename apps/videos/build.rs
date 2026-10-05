//! Compiles the AVFoundation player on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_player.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_player.m").flag("-fobjc-arc").flag("-Wno-deprecated-declarations").compile("neo_mac_player");
    for framework in ["AVFoundation", "CoreMedia", "CoreVideo", "QuartzCore", "Accelerate", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
