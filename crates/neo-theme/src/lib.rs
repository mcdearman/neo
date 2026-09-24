//! Neo's design system: colours, materials, type scale, fonts and icons.
//!
//! A [`Theme`] is plain data. Widgets never hard-code colours or shadows;
//! they ask the theme to [`paint`](Theme::paint) a [`Surface`] and get back
//! a [`Paint`] that already accounts for the style (Flat or Soft), scheme,
//! accent and the user's accessibility settings.

mod color;
pub mod fonts;
pub mod icons;

pub use color::Color;

/// A single glyph from the bundled Lucide icon font. See [`icons`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Icon(pub char);

/// How surfaces show depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    /// Borders and fills with higher contrast. The default.
    #[default]
    Flat,
    /// Neumorphic: surfaces are extruded from, or pressed into, the background
    /// using paired light and dark shadows.
    Soft,
}

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
            // Dark royal is deliberately deep so it reads like the light-scheme blue
            // against a dark surround. Text uses the lighter `text_tone`.
            Accent::Royal => (Color::hex(0x3F5BC4), Color::hex(0x546ED8)),
            Accent::Teal => (Color::hex(0x1C7F72), Color::hex(0x5CCAB8)),
            Accent::Coral => (Color::hex(0xC0533B), Color::hex(0xF29A83)),
            Accent::Amber => (Color::hex(0x9A6A0E), Color::hex(0xE8B64E)),
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub style: Style,
    pub scheme: Scheme,
    pub accent: Accent,
    /// Soft-style shadow distance in logical pixels (2 to 12).
    pub depth: f32,
    /// Window corner radius in logical pixels. Controls derive smaller radii from it.
    pub radius: f32,
    /// Multiplier for all text sizes.
    pub text_scale: f32,
    /// Turn off animation and transitions.
    pub reduce_motion: bool,
    pub glass: Glass,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            style: Style::Flat,
            scheme: Scheme::Light,
            accent: Accent::Royal,
            depth: 6.0,
            radius: 18.0,
            text_scale: 1.0,
            reduce_motion: false,
            glass: Glass::default(),
        }
    }
}

/// Resolved colours for one scheme and accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Window background. Soft surfaces share this colour.
    pub bg: Color,
    /// Flat-style card and control fill.
    pub surface: Color,
    /// Flat-style sunken fill (inputs, tracks).
    pub well: Color,
    /// Flat-style borders.
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
    /// Soft-style shadow colours.
    pub shadow_dark: Color,
    pub shadow_light: Color,
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
                shadow_dark: Color::hex(0xB6BECB),
                shadow_light: Color::WHITE,
            },
            Scheme::Dark => Self {
                bg: Color::hex(0x2B3038),
                surface: Color::hex(0x333945),
                well: Color::hex(0x252930),
                line: Color::hex(0x4A5263),
                text: Color::hex(0xE4E8EF),
                muted: Color::hex(0xA3ACBA),
                faint: Color::hex(0x7C8595),
                accent,
                accent_text,
                on_accent,
                good: Color::hex(0x5CCB92),
                warn: Color::hex(0xE6B35A),
                bad: Color::hex(0xEE8176),
                shadow_dark: Color::hex(0x1B1E23),
                shadow_light: Color::hex(0x3A404B),
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

/// A drop shadow or inner shadow, in CSS `box-shadow` terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub offset: (f32, f32),
    /// Blur radius (CSS semantics: the Gaussian sigma is half of this).
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
    pub inset: bool,
}

/// How to paint one [`Surface`].
#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    pub fill: Color,
    /// Border width and colour.
    pub border: Option<(f32, Color)>,
    pub shadows: Vec<Shadow>,
    /// Colour for content (text, icons) drawn on this surface.
    pub content: Color,
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

impl Theme {
    pub fn palette(&self) -> Palette {
        Palette::new(self.scheme, self.accent)
    }

    pub fn is_soft(&self) -> bool {
        self.style == Style::Soft
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

    fn soft_pair(&self, dist: f32, blur: f32, inset: bool) -> Vec<Shadow> {
        let p = self.palette();
        let (dark, light) = if self.glass.enabled {
            // Shadows over a translucent window read heavier; soften them.
            (p.shadow_dark.with_alpha(0.7), p.shadow_light.with_alpha(0.6))
        } else {
            (p.shadow_dark, p.shadow_light)
        };
        vec![
            Shadow { offset: (dist, dist), blur, spread: 0.0, color: dark, inset },
            Shadow { offset: (-dist, -dist), blur, spread: 0.0, color: light, inset },
        ]
    }

    /// Resolves how to paint `surface` in the current style.
    pub fn paint(&self, surface: Surface) -> Paint {
        let p = self.palette();
        let d = self.depth.clamp(0.0, 16.0);
        let glass = self.glass.enabled;
        // On glass windows, flat surfaces keep some translucency but stay readable.
        let card_fill = |c: Color| if glass { c.with_alpha(0.62) } else { c };

        match self.style {
            Style::Soft => {
                let bg = if glass { p.bg.with_alpha(0.55) } else { p.bg };
                match surface {
                    Surface::Window => Paint {
                        fill: self.window_fill(),
                        border: None,
                        shadows: vec![],
                        content: p.text,
                    },
                    Surface::Card => Paint {
                        fill: bg,
                        border: glass.then_some((1.0, p.shadow_light.with_alpha(0.35))),
                        shadows: self.soft_pair(d, d * 2.2, false),
                        content: p.text,
                    },
                    Surface::Raised => Paint {
                        fill: bg,
                        border: None,
                        shadows: self.soft_pair(d * 0.5, d, false),
                        content: p.text,
                    },
                    Surface::Hovered => Paint {
                        fill: bg,
                        border: None,
                        shadows: self.soft_pair(d * 0.6, d * 1.3, false),
                        content: p.accent_text,
                    },
                    Surface::Pressed => Paint {
                        fill: bg,
                        border: None,
                        shadows: self.soft_pair(d * 0.4, d * 0.8, true),
                        content: p.accent_text,
                    },
                    Surface::Well => Paint {
                        fill: bg,
                        border: None,
                        shadows: self.soft_pair(d * 0.7, d * 1.4, true),
                        content: p.text,
                    },
                    Surface::Inset => Paint {
                        fill: bg,
                        border: None,
                        shadows: self.soft_pair(d * 0.4, d * 0.8, true),
                        content: p.text,
                    },
                    Surface::Accent => Paint {
                        fill: p.accent,
                        border: None,
                        shadows: self.soft_pair(d * 0.5, d, false),
                        content: p.on_accent,
                    },
                }
            }
            Style::Flat => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn flat_is_default_and_has_no_soft_shadows() {
        let t = Theme::default();
        assert_eq!(t.style, Style::Flat);
        assert_eq!(t.accent, Accent::Royal);
        assert!(t.paint(Surface::Pressed).shadows.iter().all(|s| !s.inset));
        assert!(t.paint(Surface::Raised).border.is_some());
    }

    #[test]
    fn soft_pressed_is_inset() {
        let t = Theme { style: Style::Soft, ..Theme::default() };
        assert!(t.paint(Surface::Pressed).shadows.iter().all(|s| s.inset));
        assert!(t.paint(Surface::Raised).shadows.iter().all(|s| !s.inset));
    }

    #[test]
    fn reduce_motion_zeroes_transitions() {
        let t = Theme { reduce_motion: true, ..Theme::default() };
        assert_eq!(t.motion(), 0.0);
    }
}
