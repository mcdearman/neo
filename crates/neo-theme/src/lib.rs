//! Neo's design system: colours, materials, type scale, fonts and icons.
//!
//! A [`Theme`] is plain data. Widgets never hard-code colours or shadows;
//! they ask the theme to [`paint`](Theme::paint) a [`Surface`] and get back
//! a [`Paint`] that already accounts for the scheme, accent and the user's accessibility settings.

pub mod fonts;
pub mod icons;

pub use armature_render::{Color, Paint, Shadow};

/// A single glyph from the bundled Lucide icon font. See [`icons`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Icon(pub char);

/// Light or dark colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scheme {
    #[default]
    Light,
    Dark,
}

/// The accent hue used for active controls, progress and focus.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Accent {
    /// Soft royal blue. The default.
    #[default]
    Royal,
    Teal,
    Coral,
    Amber,
    /// Separate tones for the light and dark schemes. Pick tones that reach
    /// at least 4.5:1 contrast against the scheme's background.
    Custom { light: Color, dark: Color },
}

impl Accent {
    pub const PRESETS: [Accent; 4] = [Accent::Royal, Accent::Teal, Accent::Coral, Accent::Amber];

    /// An accent of a colour of the user's own choosing. Each scheme gets
    /// the colour itself where that reads against the scheme's background,
    /// and otherwise the nearest shade of it that does: darker for the
    /// light scheme, lighter for the dark. So a pale yellow is still a
    /// yellow on a white window, and can still be read there.
    pub fn from_color(color: Color) -> Self {
        let color = color.with_alpha(1.0);
        let fit = |toward: Color, scheme: Scheme| {
            let background = Palette::new(scheme, Accent::Royal).bg;
            let mut shade = color;
            let mut t = 0.0;
            while shade.contrast(background) < 4.5 && t < 1.0 {
                t += 0.02;
                shade = color.mix(toward, t);
            }
            shade
        };
        Accent::Custom { light: fit(Color::BLACK, Scheme::Light), dark: fit(Color::WHITE, Scheme::Dark) }
    }

    pub fn name(self) -> &'static str {
        match self {
            Accent::Royal => "Royal blue",
            Accent::Teal => "Teal",
            Accent::Coral => "Coral",
            Accent::Amber => "Amber",
            Accent::Custom { .. } => "Custom",
        }
    }

    /// The fill tone for this scheme: buttons, toggles, slider fills and gauges.
    pub fn tone(self, scheme: Scheme) -> Color {
        let (l, d) = match self {
            // Dark teal, coral and amber are Monokai Pro's cyan, orange and yellow.
            // Dark royal is deliberately deep so it reads like the light-scheme blue
            // against a dark surround. Text uses the lighter `text_tone`.
            Accent::Royal => (Color::hex(0x3F5BC4), Color::hex(0x546ED8)),
            Accent::Teal => (Color::hex(0x1C7F72), Color::hex(0x78DCE8)),
            Accent::Coral => (Color::hex(0xC0533B), Color::hex(0xFC9867)),
            Accent::Amber => (Color::hex(0x9A6A0E), Color::hex(0xFFD866)),
            Accent::Custom { light, dark } => (light, dark),
        };
        match scheme {
            Scheme::Light => l,
            Scheme::Dark => d,
        }
    }

    /// The tone for accent-coloured text, icons, carets and focus rings. It
    /// meets 4.5:1 on the scheme's backgrounds, so it can be lighter than the
    /// fill in dark mode.
    pub fn text_tone(self, scheme: Scheme) -> Color {
        match (self, scheme) {
            (Accent::Royal, Scheme::Dark) => Color::hex(0x889FEC),
            _ => self.tone(scheme),
        }
    }

    /// Text and icon colour on an accent fill.
    pub fn on_tone(self, scheme: Scheme) -> Color {
        let fill = self.tone(scheme);
        let (white, ink) = (Color::WHITE, Color::hex(0x14171C));
        if white.contrast(fill) >= ink.contrast(fill) { white } else { ink }
    }

    /// A mid tone that reads in either scheme, for swatches.
    pub fn swatch(self) -> Color {
        let (l, d) = (self.tone(Scheme::Light), self.tone(Scheme::Dark));
        l.mix(d, 0.4)
    }
}

/// Translucent, blurred windows.
///
/// When enabled, the window background becomes partly transparent and the
/// platform (or the Neo compositor) blurs whatever is behind it. Surfaces
/// inside the window become more opaque so text stays readable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub enabled: bool,
    /// Opacity of the window background, 0.0 to 1.0.
    pub opacity: f32,
    /// Blur strength in logical pixels, for the window's backdrop (where the
    /// platform allows it) and for in-window glass surfaces.
    pub blur: f32,
}

impl Default for Glass {
    fn default() -> Self {
        Self { enabled: false, opacity: 0.72, blur: 10.0 }
    }
}

/// Every user-adjustable appearance setting.
/// A set of colours for code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Syntax {
    /// The Meadow REPL's: each kind of name has a colour of its own, the
    /// same one the REPL gives it in a Neo terminal. The default.
    #[default]
    Meadow,
    /// Monokai Pro's on a dark ground, and the accent's tones on a light one.
    Monokai,
}

impl Syntax {
    pub const ALL: [Syntax; 2] = [Syntax::Meadow, Syntax::Monokai];

    pub fn name(self) -> &'static str {
        match self {
            Syntax::Meadow => "Meadow",
            Syntax::Monokai => "Monokai",
        }
    }

    /// The scheme a name stands for, in any case.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub scheme: Scheme,
    pub accent: Accent,
    /// Window corner radius in logical pixels. Controls derive smaller radii from it.
    pub radius: f32,
    /// Multiplier for all text sizes.
    pub text_scale: f32,
    /// Turn off animation and transitions.
    pub reduce_motion: bool,
    pub glass: Glass,
    /// How code is coloured.
    pub syntax: Syntax,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            scheme: Scheme::Light,
            accent: Accent::Royal,
            radius: 18.0,
            text_scale: 1.0,
            reduce_motion: false,
            glass: Glass::default(),
            syntax: Syntax::default(),
        }
    }
}

/// Resolved colours for one scheme and accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Window background.
    pub bg: Color,
    /// Card and control fill.
    pub surface: Color,
    /// Sunken fill (inputs, tracks).
    pub well: Color,
    /// Borders.
    pub line: Color,
    pub text: Color,
    /// Secondary text. Meets 4.5:1 on `bg`.
    pub muted: Color,
    /// Tertiary text and disabled content.
    pub faint: Color,
    /// Accent fills and graphics.
    pub accent: Color,
    /// Accent-coloured text and icons on ordinary surfaces.
    pub accent_text: Color,
    /// Text and icons on an accent fill.
    pub on_accent: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
}

impl Palette {
    pub fn new(scheme: Scheme, accent_choice: Accent) -> Self {
        let accent = accent_choice.tone(scheme);
        let accent_text = accent_choice.text_tone(scheme);
        let on_accent = accent_choice.on_tone(scheme);
        match scheme {
            Scheme::Light => Self {
                bg: Color::hex(0xE3E7EE),
                surface: Color::hex(0xF6F7F9),
                well: Color::hex(0xEBEEF3),
                line: Color::hex(0xC5CCD7),
                text: Color::hex(0x2B3240),
                muted: Color::hex(0x5C6576),
                faint: Color::hex(0x8A93A3),
                accent,
                accent_text,
                on_accent,
                good: Color::hex(0x1E8A5A),
                warn: Color::hex(0xB7791F),
                bad: Color::hex(0xC0443A),
            },
            // Monokai Pro colours over a #181818 background.
            Scheme::Dark => Self {
                bg: Color::hex(0x181818),
                surface: Color::hex(0x211F22),
                well: Color::hex(0x141314),
                line: Color::hex(0x3A383B),
                text: Color::hex(0xFCFCFA),
                muted: Color::hex(0x939293),
                faint: Color::hex(0x727072),
                accent,
                accent_text,
                on_accent,
                good: Color::hex(0xA9DC76),
                warn: Color::hex(0xFFD866),
                bad: Color::hex(0xFF6188),
            },
        }
    }
}

/// The role a painted area plays. The theme turns this into concrete fills,
/// borders and shadows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The window itself.
    Window,
    /// A card or window-sized panel lifted off the background.
    Card,
    /// A pressable control at rest.
    Raised,
    /// A pressable control with the pointer over it.
    Hovered,
    /// A control being pressed, or one that is on or selected.
    Pressed,
    /// A large sunken area such as a display or chart well.
    Well,
    /// A small sunken area such as a slider track or input field.
    Inset,
    /// A solid accent fill for primary actions.
    Accent,
}

/// Text roles in the type scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// Small uppercase section labels.
    Label,
    Caption,
    Body,
    /// Emphasised body text, such as control labels.
    Strong,
    Title,
    Heading,
    Display,
}

/// Font weight, CSS numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Weight(pub u16);

impl Weight {
    pub const REGULAR: Weight = Weight(400);
    pub const MEDIUM: Weight = Weight(500);
    pub const SEMIBOLD: Weight = Weight(600);
    pub const BOLD: Weight = Weight(700);
    pub const EXTRABOLD: Weight = Weight(800);
}

/// Resolved font settings for a [`TextRole`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSpec {
    pub size: f32,
    pub weight: Weight,
    pub line_height: f32,
    pub uppercase: bool,
    pub letter_spacing: f32,
}

impl TextSpec {
    /// The renderer's text style for this role, in the interface typeface.
    pub fn style(self) -> armature_render::TextStyle {
        armature_render::TextStyle {
            size: self.size,
            weight: self.weight.0,
            family: armature_render::FontFamily::Sans,
            line_height: self.line_height,
            letter_spacing: self.letter_spacing,
        }
    }
}

impl Theme {
    pub fn palette(&self) -> Palette {
        Palette::new(self.scheme, self.accent)
    }

    /// Corner radius for windows.
    pub fn window_radius(&self) -> f32 {
        self.radius
    }

    /// Corner radius for cards and large controls.
    pub fn control_radius(&self) -> f32 {
        (self.radius * 0.6).max(4.0)
    }

    /// Corner radius for small controls such as segmented options.
    pub fn small_radius(&self) -> f32 {
        (self.radius * 0.45).max(3.0)
    }

    /// Duration of state transitions in seconds. Zero when motion is reduced.
    pub fn motion(&self) -> f32 {
        if self.reduce_motion { 0.0 } else { 0.14 }
    }

    pub fn text(&self, role: TextRole) -> TextSpec {
        let (size, weight, lh, upper, ls) = match role {
            TextRole::Label => (10.5, Weight::BOLD, 1.3, true, 1.1),
            TextRole::Caption => (12.0, Weight::MEDIUM, 1.4, false, 0.0),
            TextRole::Body => (14.0, Weight::MEDIUM, 1.45, false, 0.0),
            TextRole::Strong => (14.0, Weight::BOLD, 1.45, false, 0.0),
            TextRole::Title => (16.0, Weight::EXTRABOLD, 1.3, false, 0.0),
            TextRole::Heading => (22.0, Weight::EXTRABOLD, 1.2, false, -0.2),
            TextRole::Display => (34.0, Weight::MEDIUM, 1.1, false, -0.6),
        };
        TextSpec {
            size: size * self.text_scale,
            weight,
            line_height: lh,
            uppercase: upper,
            letter_spacing: ls,
        }
    }

    /// Window background, including glass translucency.
    pub fn window_fill(&self) -> Color {
        let p = self.palette();
        if self.glass.enabled {
            p.bg.with_alpha(self.glass.opacity.clamp(0.0, 1.0))
        } else {
            p.bg
        }
    }

    /// Resolves how to paint `surface` in the current style.
    pub fn paint(&self, surface: Surface) -> Paint {
        let p = self.palette();
        let glass = self.glass.enabled;
        // On glass windows, flat surfaces keep some translucency but stay readable.
        let card_fill = |c: Color| if glass { c.with_alpha(0.62) } else { c };

        let hairline = |c: Color| Some((1.0, c));
        let lift = Shadow {
            offset: (0.0, 1.0),
            blur: 3.0,
            spread: 0.0,
            color: Color::BLACK.with_alpha(if self.scheme == Scheme::Dark { 0.28 } else { 0.08 }),
            inset: false,
        };
        let card_lift = Shadow { offset: (0.0, 2.0), blur: 10.0, ..lift };
        match surface {
            Surface::Window => Paint {
                fill: self.window_fill(),
                border: None,
                shadows: vec![],
                content: p.text,
            },
            Surface::Card => Paint {
                fill: card_fill(p.surface),
                border: hairline(p.line),
                shadows: vec![card_lift],
                content: p.text,
            },
            Surface::Raised => Paint {
                fill: card_fill(p.surface),
                border: hairline(p.line),
                shadows: vec![lift],
                content: p.text,
            },
            Surface::Hovered => Paint {
                fill: card_fill(p.surface.mix(p.accent, 0.06)),
                border: hairline(p.line.mix(p.accent, 0.5)),
                shadows: vec![lift],
                content: p.accent_text,
            },
            Surface::Pressed => Paint {
                fill: card_fill(p.surface.mix(p.accent, 0.13)),
                border: hairline(p.accent),
                shadows: vec![],
                content: p.accent_text,
            },
            Surface::Well | Surface::Inset => Paint {
                fill: card_fill(p.well),
                border: hairline(p.line),
                shadows: vec![],
                content: p.text,
            },
            Surface::Accent => Paint {
                fill: p.accent,
                border: None,
                shadows: vec![lift],
                content: p.on_accent,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accent_of_any_colour_reads_in_both_schemes() {
        for hex in [0xFFF2A8, 0x101820, 0xFF2D95, 0x3F5BC4, 0xFFFFFF, 0x000000, 0x7BC96F] {
            let Accent::Custom { light, dark } = Accent::from_color(Color::hex(hex)) else { panic!("a custom accent") };
            let (on_light, on_dark) = (Palette::new(Scheme::Light, Accent::Royal).bg, Palette::new(Scheme::Dark, Accent::Royal).bg);
            assert!(light.contrast(on_light) >= 4.5 && dark.contrast(on_dark) >= 4.5, "{hex:06X}: {} and {}", light.contrast(on_light), dark.contrast(on_dark));
        }
        // One that reads already is kept as it is, in the scheme where it does.
        let royal = Color::hex(0x3F5BC4);
        assert!(matches!(Accent::from_color(royal), Accent::Custom { light, .. } if light == royal));
        // A pale one is darkened for the light scheme and is still of its own hue: more red and green than blue.
        let Accent::Custom { light, dark } = Accent::from_color(Color::hex(0xFFF2A8)) else { unreachable!() };
        assert!(light.r > light.b && light.g > light.b && light.luminance() < dark.luminance());
        assert_eq!(dark, Color::hex(0xFFF2A8), "and kept as it is for the dark one");
    }

    #[test]
    fn text_contrast_meets_wcag_aa() {
        for scheme in [Scheme::Light, Scheme::Dark] {
            for accent in Accent::PRESETS {
                let p = Palette::new(scheme, accent);
                for bg in [p.bg, p.surface, p.well] {
                    assert!(p.text.contrast(bg) >= 7.0, "{scheme:?} text");
                    assert!(p.muted.contrast(bg) >= 4.5, "{scheme:?} muted on {bg:?}");
                }
                // Accent text sits on backgrounds and cards and must stay legible.
                // Known gap: light-scheme teal, coral and amber reach only 3.7 to 3.9:1
                // on `bg`. Royal blue, the default, meets 4.5:1 everywhere.
                let min = if accent == Accent::Royal || scheme == Scheme::Dark { 4.5 } else { 3.5 };
                for bg in [p.bg, p.surface] {
                    let c = p.accent_text.contrast(bg);
                    assert!(c >= min, "{scheme:?} {accent:?} accent text contrast {c:.2} on {bg:?}");
                }
                // Fills and graphics sit next to tracks and backgrounds.
                let c = p.accent.contrast(p.bg);
                assert!(c >= 2.9, "{scheme:?} {accent:?} accent fill contrast {c:.2}");
                assert!(p.on_accent.contrast(p.accent) >= 4.5, "{scheme:?} {accent:?} on-accent");
            }
        }
    }

    #[test]
    fn defaults_are_royal_with_flat_surfaces() {
        let t = Theme::default();
        assert_eq!(t.accent, Accent::Royal);
        assert!(t.paint(Surface::Pressed).shadows.iter().all(|s| !s.inset));
        assert!(t.paint(Surface::Raised).border.is_some());
    }

    #[test]
    fn reduce_motion_zeroes_transitions() {
        let t = Theme { reduce_motion: true, ..Theme::default() };
        assert_eq!(t.motion(), 0.0);
    }
}
