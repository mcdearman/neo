use neo_render::Size;

use crate::core::{Cx, DrawCx, Length, Limits, Widget};
use crate::ThemeCx;

/// A thin separating line.
pub struct Divider {
    vertical: bool,
}

impl Divider {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }

    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl<M> Widget<M> for Divider {
    fn width(&self) -> Length {
        if self.vertical { Length::Shrink } else { Length::Fill }
    }

    fn height(&self) -> Length {
        if self.vertical { Length::Fill } else { Length::Shrink }
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        let t = 1.0;
        let l = limits.constrain(Widget::<M>::width(self), Widget::<M>::height(self));
        if self.vertical { l.resolve(Size::new(t, 0.0)) } else { l.resolve(Size::new(0.0, t)) }
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let line = if self.vertical { neo_render::Rect::new(b.x, b.y, 1.0, b.h) } else { neo_render::Rect::new(b.x, b.y, b.w, 1.0) };
        cx.scene.fill(line, 0.0, p.line, None);
    }
}
