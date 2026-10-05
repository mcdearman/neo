//! Bundled typefaces. All are licensed under the SIL Open Font License 1.1
//! (Manrope, JetBrains Mono) or ISC (Lucide); licence texts live in `assets/`.

/// Family name of the interface typeface.
pub const SANS: &str = "Manrope";
/// Family name of the monospace typeface, used for numbers and code.
pub const MONO: &str = "JetBrains Mono";
/// Family name of the icon font.
pub const ICONS: &str = "lucide";

/// Every bundled font file, ready to hand to a font database.
pub const ALL: &[&[u8]] = &[
    include_bytes!("../assets/manrope-400.ttf"),
    include_bytes!("../assets/manrope-500.ttf"),
    include_bytes!("../assets/manrope-600.ttf"),
    include_bytes!("../assets/manrope-700.ttf"),
    include_bytes!("../assets/manrope-800.ttf"),
    include_bytes!("../assets/jetbrains-mono-400.ttf"),
    include_bytes!("../assets/jetbrains-mono-500.ttf"),
    include_bytes!("../assets/lucide.ttf"),
];

/// The bundled typefaces, for [`neo_render::Renderer::new`].
pub fn bundled() -> neo_render::Fonts {
    neo_render::Fonts {
        data: ALL.iter().map(|d| std::borrow::Cow::Borrowed(*d)).collect(),
        sans: SANS.into(),
        mono: MONO.into(),
        icons: ICONS.into(),
    }
}
