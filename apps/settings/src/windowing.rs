//! A picture of how tiled windows would share a screen, for the Windows page.

use neo::prelude::*;
use neo::{Cx, DrawCx, Limits, Rect, Size, Widget};
use neo_desktop::tiling::{Layout, Windowing};

/// The screen the picture is of: its shape, and the size the settings'
/// gaps are measured against.
const SCREEN: Size = Size { w: 1440.0, h: 900.0 };

/// A small screen with `windows` windows laid out as the settings say.
pub struct Preview {
    settings: Windowing,
    windows: usize,
    width: f32,
}

impl Preview {
    pub fn new(settings: &Windowing, windows: usize, width: f32) -> Self {
        Self { settings: settings.clone(), windows, width }
    }
}

/// Where each window is drawn in a picture `into`, the main ones first.
pub fn places(settings: &Windowing, windows: usize, into: Rect) -> Vec<Rect> {
    let by = into.w / SCREEN.w;
    settings.arrange(windows, Rect::new(0.0, 0.0, SCREEN.w, SCREEN.h)).into_iter().map(|r| Rect::new((into.x + r.x * by).round(), (into.y + r.y * by).round(), (r.w * by).round().max(2.0), (r.h * by).round().max(2.0))).collect()
}

impl<M: 'static> Widget<M> for Preview {
    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        let w = self.width.min(limits.max.w);
        Size::new(w, (w * SCREEN.h / SCREEN.w).round())
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        let (line, accent, bg, faint) = (p.line, p.accent, p.bg, p.faint);
        cx.scene.fill(b, 6.0, bg, Some((1.0, line)));
        let masters = if self.settings.layout == Layout::MasterStack { self.settings.masters as usize } else { 1 };
        let places = places(&self.settings, self.windows, b);
        // One at a time: those behind show as a edge at the back.
        let monocle = self.settings.layout == Layout::Monocle;
        for (i, r) in places.iter().enumerate().rev() {
            let r = if monocle { Rect::new(r.x + i as f32 * 3.0, r.y + i as f32 * 3.0, (r.w - i as f32 * 6.0).max(2.0), (r.h - i as f32 * 6.0).max(2.0)) } else { *r };
            let main = i < masters;
            cx.scene.fill(r, 3.0, if main { accent.with_alpha(0.85) } else { faint.with_alpha(0.28) }, Some((1.0, if main { accent } else { line })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_the_layout_made_small() {
        let into = Rect::new(100.0, 50.0, 360.0, 225.0);
        let settings = Windowing { gap: 8.0, margin: 8.0, ..Windowing::default() };
        let three = places(&settings, 3, into);
        assert_eq!(three.len(), 3);
        assert!(three.iter().all(|r| r.x >= into.x && r.y >= into.y && r.right() <= into.right() + 1.0 && r.bottom() <= into.bottom() + 1.0), "inside the picture: {three:?}");
        assert!(three[0].w > three[1].w && three[1].x > three[0].right() && three[2].y > three[1].bottom(), "the main one on the left, two stacked on the right");
        assert_eq!(places(&Windowing { layout: Layout::Columns, ..settings.clone() }, 4, into).iter().map(|r| r.y).collect::<Vec<_>>(), vec![three[0].y; 4]);
        assert!(places(&settings, 0, into).is_empty());
    }
}
