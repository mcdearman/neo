use armature_render::{FontFamily, Point, Size, TextLayout, TextStyle};

use super::style::Tone;
use crate::ThemeCx;
use crate::core::{Cx, DrawCx, Limits, Widget};

/// A glyph from the Lucide icon set. See [`neo_theme::icons`].
pub struct Icon {
    icon: neo_theme::Icon,
    size: f32,
    tone: Tone,
    layout: Option<TextLayout>,
}

impl Icon {
    pub fn new(icon: neo_theme::Icon) -> Self {
        Self { icon, size: 18.0, tone: Tone::Inherit, layout: None }
    }

    pub fn size(mut self, s: f32) -> Self {
        self.size = s;
        self
    }

    pub fn tone(mut self, t: Tone) -> Self {
        self.tone = t;
        self
    }
}

/// Shorthand for [`Icon::new`].
pub fn icon(icon: neo_theme::Icon) -> Icon {
    Icon::new(icon)
}

impl<M> Widget<M> for Icon {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = self.size * cx.theme().text_scale;
        let style = TextStyle { size, weight: 400, family: FontFamily::Icons, line_height: 1.0, letter_spacing: 0.0 };
        self.layout = Some(cx.text().layout(&self.icon.0.to_string(), &style, None));
        limits.resolve(Size::new(size, size))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let Some(layout) = &self.layout else { return };
        let b = cx.bounds();
        let s = layout.size();
        let color = self.tone.resolve(cx.content_color(), &cx.theme().palette());
        cx.scene.text(layout, Point::new(b.x + (b.w - s.w) * 0.5, b.y + (b.h - s.h) * 0.5), color);
    }
}
