use neo_render::{FontFamily, Point, Size, TextLayout};
use neo_theme::{TextRole, Weight};

use super::style::Tone;
use crate::core::{Align, Cx, DrawCx, Length, Limits, Widget};

/// A run of text in one style.
pub struct Text {
    content: String,
    role: TextRole,
    tone: Tone,
    family: FontFamily,
    weight: Option<Weight>,
    size: Option<f32>,
    width: Length,
    align: Align,
    wrap: bool,
    layout: Option<TextLayout>,
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            role: TextRole::Body,
            tone: Tone::Inherit,
            family: FontFamily::Sans,
            weight: None,
            size: None,
            width: Length::Shrink,
            align: Align::Start,
            wrap: true,
            layout: None,
        }
    }

    pub fn role(mut self, role: TextRole) -> Self {
        self.role = role;
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Uses the monospace face with tabular figures.
    pub fn mono(mut self) -> Self {
        self.family = FontFamily::Mono;
        self
    }

    pub fn weight(mut self, w: Weight) -> Self {
        self.weight = Some(w);
        self
    }

    /// Overrides the role's size (before the user's text scale is applied).
    pub fn size(mut self, s: f32) -> Self {
        self.size = Some(s);
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    /// Horizontal alignment within the text's box.
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }

    /// Keep on one line even when space runs out.
    pub fn no_wrap(mut self) -> Self {
        self.wrap = false;
        self
    }
}

/// Shorthand for [`Text::new`].
pub fn text(content: impl Into<String>) -> Text {
    Text::new(content)
}

impl<M> Widget<M> for Text {
    fn width(&self) -> Length {
        self.width
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let theme = *cx.theme();
        let spec = theme.text(self.role);
        let mut style = spec.style();
        style.family = self.family;
        if let Some(w) = self.weight {
            style.weight = w.0;
        }
        if let Some(s) = self.size {
            style.size = s * theme.text_scale;
        }
        if self.family == FontFamily::Mono {
            style.letter_spacing = 0.0;
            style.weight = style.weight.min(500);
        }
        let content = if spec.uppercase { self.content.to_uppercase() } else { self.content.clone() };
        let limits = limits.constrain(self.width, Length::Shrink);
        let max_w = if self.wrap { Some(limits.max.w) } else { None };
        let mut layout = cx.text().layout(&content, &style, max_w);
        // One-line text that is too long for its space is cut short with an
        // ellipsis, so it cannot run over whatever sits beside it.
        if !self.wrap && limits.max.w.is_finite() && layout.size().w > limits.max.w {
            let chars: Vec<char> = content.chars().collect();
            // The longest prefix that fits with the ellipsis, by bisection.
            let (mut fits, mut too_long) = (0, chars.len());
            while fits + 1 < too_long {
                let mid = (fits + too_long) / 2;
                let candidate: String = chars[..mid].iter().collect::<String>().trim_end().to_string() + "…";
                if cx.text().layout(&candidate, &style, None).size().w <= limits.max.w {
                    fits = mid;
                } else {
                    too_long = mid;
                }
            }
            let shortened = if fits == 0 { "…".to_string() } else { chars[..fits].iter().collect::<String>().trim_end().to_string() + "…" };
            layout = cx.text().layout(&shortened, &style, None);
        }
        let size = limits.resolve(layout.size());
        self.layout = Some(layout);
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let Some(layout) = &self.layout else { return };
        let b = cx.bounds();
        let x = b.x + self.align.offset(b.w - layout.size().w);
        let color = self.tone.resolve(cx.content_color(), &cx.theme().palette());
        cx.scene.text(layout, Point::new(x, b.y), color);
    }
}
