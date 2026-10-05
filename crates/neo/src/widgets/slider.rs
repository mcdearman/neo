use std::ops::RangeInclusive;

use armature_render::{Rect, Size};
use neo_theme::Surface;

use super::style::lerp_paint;
use crate::{FocusRing, ThemeCx};
use crate::anim::Anim;
use armature::controls::{SliderLogic, SliderState};

use crate::core::{Cx, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Status};

/// What the painting keeps between frames; the behaviour's own state is
/// the framework's [`SliderState`].
#[derive(Default)]
struct SliderLook {
    hover: Anim,
}

/// Picks a number from a range by dragging a thumb along a track.
pub struct Slider<M> {
    logic: SliderLogic,
    on_change: Box<dyn Fn(f32) -> M>,
    on_release: Option<M>,
    width: Length,
}

impl<M: Clone> Slider<M> {
    pub fn new(range: RangeInclusive<f32>, value: f32, on_change: impl Fn(f32) -> M + 'static) -> Self {
        Self { logic: SliderLogic::new(range, value), on_change: Box::new(on_change), on_release: None, width: Length::Fill }
    }

    /// Snap to multiples of `step` (also the keyboard increment).
    pub fn step(mut self, s: f32) -> Self {
        self.logic.step = Some(s);
        self
    }

    /// Sent when the user lets go of the thumb.
    pub fn on_release(mut self, m: M) -> Self {
        self.on_release = Some(m);
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }
}

/// Shorthand for [`Slider::new`].
pub fn slider<M: Clone>(range: RangeInclusive<f32>, value: f32, on_change: impl Fn(f32) -> M + 'static) -> Slider<M> {
    Slider::new(range, value, on_change)
}

const THUMB: f32 = 22.0;
const TRACK: f32 = 10.0;

impl<M: Clone + 'static> Widget<M> for Slider<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        let l = limits.constrain(self.width, Length::Shrink);
        l.resolve(Size::new(if l.max.w.is_finite() { l.max.w } else { 200.0 }, THUMB + 6.0))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let now = cx.now();
        let active = {
            let st = cx.state::<SliderState>();
            st.hovered || st.dragging
        };
        let (h, anim) = {
            let look = cx.state::<SliderLook>();
            let h = look.hover.step(if active { 1.0 } else { 0.0 }, now, theme.motion());
            (h, look.hover.is_animating())
        };
        if anim {
            cx.request_animation();
        }
        let track = Rect::new(b.x, b.y + (b.h - TRACK) * 0.5, b.w, TRACK);
        cx.scene.paint(track, TRACK * 0.5, &theme.paint(Surface::Inset));
        let usable = b.w - THUMB;
        let cxpos = b.x + THUMB * 0.5 + usable * self.logic.fraction();
        let fill = Rect::new(track.x + 2.0, track.y + 2.0, (cxpos - track.x - 2.0).max(TRACK - 4.0), TRACK - 4.0);
        cx.scene.fill(fill, (TRACK - 4.0) * 0.5, p.accent, None);
        let thumb = Rect::new(cxpos - THUMB * 0.5, b.y + (b.h - THUMB) * 0.5, THUMB, THUMB);
        let paint = lerp_paint(&theme.paint(Surface::Raised), &theme.paint(Surface::Hovered), h);
        cx.scene.paint(thumb, THUMB * 0.5, &paint);
        cx.scene.fill(thumb.inset(7.0), 4.0, p.accent.with_alpha(0.25 + 0.75 * h), None);
        cx.focus_ring(thumb, THUMB * 0.5);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        // The thumb's centre travels between half a thumb in from each end.
        let (status, change) = self.logic.event(cx, event, b, b.x + THUMB * 0.5, b.w - THUMB);
        if let Some(v) = change.value {
            cx.emit((self.on_change)(v));
        }
        if change.released
            && let Some(m) = self.on_release.clone()
        {
            cx.emit(m);
        }
        status
    }
}
