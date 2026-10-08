use armature_render::{Point, Rect, Size};

use armature::controls::{ScrollLogic, ScrollState, ScrollStep};

use crate::ThemeCx;
use crate::anim::Anim;
use crate::core::{Cx, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, Status};

/// What the painting keeps between frames; the offset and drag live in
/// the framework's [`ScrollState`].
#[derive(Default)]
struct ScrollLook {
    hover: Anim,
    /// The last request to bring something into view that was met.
    revealed: u64,
}

/// Scrolls its child vertically.
pub struct Scrollable<M> {
    child: [Element<M>; 1],
    width: Length,
    height: Length,
    logic: ScrollLogic,
    /// A request to bring one of the content's children into view: its
    /// number, and which child.
    reveal: Option<(u64, usize)>,
}

impl<M: 'static> Scrollable<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self { child: [child.into()], width: Length::Fill, height: Length::Fill, logic: ScrollLogic::default(), reveal: None }
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }

    /// Scrolls so that the content's child at `index` is in view, once
    /// for each new `serial`: change the number to ask again. For a list
    /// whose selection has moved somewhere that may be out of sight. Zero
    /// asks for nothing.
    pub fn reveal(mut self, serial: u64, index: usize) -> Self {
        self.reveal = (serial != 0).then_some((serial, index));
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
        self.logic.thumb(b, offset, BAR, 3.0, 28.0)
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
        self.logic.content = cs.h;
        let size = l.resolve(cs);
        // Asked to show one of the rows, and not yet done: move just far
        // enough that it is inside, with a little room around it.
        if let Some((serial, index)) = self.reveal
            && std::mem::replace(&mut cx.state::<ScrollLook>().revealed, serial) != serial
            && let Some(row) = self.child[0].children_mut().get(index)
        {
            let (top, bottom) = (row.position().y, row.position().y + row.bounds().h);
            let st = cx.state::<ScrollState>();
            let room = 8.0;
            if top - room < st.offset {
                st.offset = top - room;
            } else if bottom + room > st.offset + size.h {
                st.offset = bottom + room - size.h;
            }
        }
        let off = self.logic.clamp(cx, size.h);
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
        let (offset, active) = {
            let st = cx.state::<ScrollState>();
            (st.offset, st.hovered || st.drag.is_some())
        };
        let (h, anim) = {
            let look = cx.state::<ScrollLook>();
            let h = look.hover.step(if active { 1.0 } else { 0.0 }, now, theme.motion());
            (h, look.hover.is_animating())
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
        if let Event::Wheel { pos, delta } = event
            && b.contains(*pos)
        {
            // Whatever is inside gets the wheel first, so nested scrolling works.
            let status = self.child[0].event(cx, event);
            if status == Status::Captured || !self.logic.wheel(cx, delta.y, b.h) {
                return status;
            }
            return Status::Captured;
        }
        let offset = cx.state::<ScrollState>().offset;
        match self.logic.pointer(cx, event, b, self.thumb(b, offset)) {
            ScrollStep::Done(status) => status,
            ScrollStep::Pass => self.child[0].event(cx, event),
            ScrollStep::PassAway => self.child[0].event(cx, &Event::PointerMoved { pos: Point::new(f32::MIN, f32::MIN) }),
        }
    }
}
