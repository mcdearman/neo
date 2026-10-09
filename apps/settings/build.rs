//! Compiles the desktop-picture helper on macOS.

fn main() {
    println!("cargo:rerun-if-changed=src/mac_wallpaper.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new().file("src/mac_wallpaper.m").flag("-fobjc-arc").compile("neo_mac_wallpaper");
    for framework in ["AppKit", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
