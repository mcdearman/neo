use armature_render::{Point, Rect, Size};

use crate::ThemeCx;
use crate::anim::Anim;
use crate::core::{Cx, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, PointerButton, Status};

#[derive(Default)]
struct ScrollState {
    offset: f32,
    drag: Option<(f32, f32)>,
    hover: Anim,
    hovered: bool,
}

/// Scrolls its child vertically.
pub struct Scrollable<M> {
    child: [Element<M>; 1],
    width: Length,
    height: Length,
    content: f32,
}

impl<M: 'static> Scrollable<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self { child: [child.into()], width: Length::Fill, height: Length::Fill, content: 0.0 }
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }
}

/// Shorthand for [`Scrollable::new`].
pub fn scrollable<M: 'static>(child: impl Into<Element<M>>) -> Scrollable<M> {
    Scrollable::new(child)
}

const BAR: f32 = 6.0;

impl<M: 'static> Scrollable<M> {
    fn thumb(&self, b: Rect, offset: f32) -> Option<Rect> {
        if self.content <= b.h + 0.5 {
            return None;
        }
        let h = (b.h * b.h / self.content).max(28.0);
        let max_off = self.content - b.h;
        let y = b.y + (b.h - h) * (offset / max_off);
        Some(Rect::new(b.right() - BAR - 3.0, y, BAR, h))
    }
}

impl<M: 'static> Widget<M> for Scrollable<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn height(&self) -> Length {
        self.height
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let l = limits.constrain(self.width, self.height);
        let cs = self.child[0].layout(cx, Limits::new(Size::new(l.min.w, 0.0), Size::new(l.max.w, f32::INFINITY)));
        self.content = cs.h;
        let size = l.resolve(cs);
        let max_off = (cs.h - size.h).max(0.0);
        let st = cx.state::<ScrollState>();
        st.offset = st.offset.clamp(0.0, max_off);
        let off = st.offset;
        self.child[0].set_position(Point::new(0.0, -off));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let now = cx.now();
        cx.scene.push_clip(b);
        self.child[0].draw(cx);
        cx.scene.pop_clip();
        let (offset, h, anim) = {
            let st = cx.state::<ScrollState>();
            let active = st.hovered || st.drag.is_some();
            let h = st.hover.step(if active { 1.0 } else { 0.0 }, now, theme.motion());
            (st.offset, h, st.hover.is_animating())
        };
        if anim {
            cx.request_animation();
        }
        if let Some(t) = self.thumb(b, offset) {
            let c = theme.palette().faint.with_alpha(0.35 + 0.4 * h);
            cx.scene.fill(t, BAR * 0.5, c, None);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let max_off = (self.content - b.h).max(0.0);
        match event {
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let status = self.child[0].event(cx, event);
                if status == Status::Captured || max_off <= 0.0 {
                    return status;
                }
                let st = cx.state::<ScrollState>();
                let new = (st.offset + delta.y).clamp(0.0, max_off);
                if new != st.offset {
                    st.offset = new;
                    cx.request_layout();
                }
                return Status::Captured;
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => {
                let offset = cx.state::<ScrollState>().offset;
                if let Some(t) = self.thumb(b, offset)
                    && t.inset(-4.0).contains(*pos) {
                        cx.state::<ScrollState>().drag = Some((pos.y, offset));
                        return Status::Captured;
                    }
                if !b.contains(*pos) {
                    return Status::Ignored;
                }
            }
            Event::PointerMoved { pos } => {
                let hovered = b.contains(*pos);
                let (changed, drag, offset) = {
                    let st = cx.state::<ScrollState>();
                    let changed = st.hovered != hovered;
                    st.hovered = hovered;
                    (changed, st.drag, st.offset)
                };
                if changed {
                    cx.request_redraw();
                }
                if let Some((y0, off0)) = drag {
                    let track = b.h - self.thumb(b, offset).map_or(b.h, |t| t.h);
                    if track > 0.0 {
                        cx.state::<ScrollState>().offset = (off0 + (pos.y - y0) * max_off / track).clamp(0.0, max_off);
                        cx.request_layout();
                    }
                    return Status::Captured;
                }
                if !hovered {
                    // Keep scrolled-away children from reacting to a pointer outside the viewport.
                    return self.child[0].event(cx, &Event::PointerMoved { pos: Point::new(f32::MIN, f32::MIN) });
                }
            }
            Event::PointerReleased { .. } => {
                cx.state::<ScrollState>().drag = None;
            }
            _ => {}
        }
        self.child[0].event(cx, event)
    }
}
