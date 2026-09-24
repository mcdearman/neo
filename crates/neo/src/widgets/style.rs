use neo_theme::{Color, Paint, Palette, Shadow};

/// Semantic colours for text and icons.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tone {
    /// Follow the surface underneath (see `Cx::content_color`).
    #[default]
    Inherit,
    Muted,
    Faint,
    Accent,
    Good,
    Warn,
    Bad,
    Custom(Color),
}

impl Tone {
    pub fn resolve(self, inherited: Color, p: &Palette) -> Color {
        match self {
            Tone::Inherit => inherited,
            Tone::Muted => p.muted,
            Tone::Faint => p.faint,
            Tone::Accent => p.accent_text,
            Tone::Good => p.good,
            Tone::Warn => p.warn,
            Tone::Bad => p.bad,
            Tone::Custom(c) => c,
        }
    }
}

/// Blends two paints for state transitions. Shadows cross-fade, so a soft
/// control's outer shadows fade out while its inner shadows fade in.
pub fn lerp_paint(a: &Paint, b: &Paint, t: f32) -> Paint {
    if t <= 0.0 {
        return a.clone();
    }
    if t >= 1.0 {
        return b.clone();
    }
    let fade = |s: &Shadow, k: f32| Shadow { color: s.color.with_alpha(s.color.a * k), ..*s };
    let mut shadows: Vec<Shadow> = a.shadows.iter().map(|s| fade(s, 1.0 - t)).collect();
    shadows.extend(b.shadows.iter().map(|s| fade(s, t)));
    let border = match (a.border, b.border) {
        (Some((wa, ca)), Some((wb, cb))) => Some((wa + (wb - wa) * t, ca.mix(cb, t))),
        (Some((w, c)), None) => Some((w, c.with_alpha(c.a * (1.0 - t)))),
        (None, Some((w, c))) => Some((w, c.with_alpha(c.a * t))),
        (None, None) => None,
    };
    Paint { fill: a.fill.mix(b.fill, t), border, shadows, content: a.content.mix(b.content, t) }
}
