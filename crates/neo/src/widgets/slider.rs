use std::ops::RangeInclusive;

use neo_render::{Rect, Size};
use neo_theme::Surface;

use super::style::lerp_paint;
use crate::anim::Anim;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct SliderState {
    dragging: bool,
    hovered: bool,
    hover: Anim,
}

/// Picks a number from a range by dragging a thumb along a track.
pub struct Slider<M> {
    range: RangeInclusive<f32>,
    value: f32,
    step: Option<f32>,
    on_change: Box<dyn Fn(f32) -> M>,
    on_release: Option<M>,
    width: Length,
}

impl<M: Clone> Slider<M> {
    pub fn new(range: RangeInclusive<f32>, value: f32, on_change: impl Fn(f32) -> M + 'static) -> Self {
        Self { range, value, step: None, on_change: Box::new(on_change), on_release: None, width: Length::Fill }
    }

    /// Snap to multiples of `step` (also the keyboard increment).
    pub fn step(mut self, s: f32) -> Self {
        self.step = Some(s);
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

    fn snap(&self, v: f32) -> f32 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let v = v.clamp(lo, hi);
        match self.step {
            Some(s) if s > 0.0 => (lo + ((v - lo) / s).round() * s).clamp(lo, hi),
            _ => v,
        }
    }

    fn fraction(&self) -> f32 {
        let (lo, hi) = (*self.range.start(), *self.range.end());
        if hi > lo { ((self.value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 }
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
        let (h, anim) = {
            let st = cx.state::<SliderState>();
            let active = st.hovered || st.dragging;
            let h = st.hover.step(if active { 1.0 } else { 0.0 }, now, theme.motion());
            (h, st.hover.is_animating())
        };
        if anim {
            cx.request_animation();
        }
        let track = Rect::new(b.x, b.y + (b.h - TRACK) * 0.5, b.w, TRACK);
        cx.scene.paint(track, TRACK * 0.5, &theme.paint(Surface::Inset));
        let usable = b.w - THUMB;
        let cxpos = b.x + THUMB * 0.5 + usable * self.fraction();
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
        let value_at = |x: f32| {
            let t = ((x - b.x - THUMB * 0.5) / (b.w - THUMB).max(1.0)).clamp(0.0, 1.0);
            *self.range.start() + t * (*self.range.end() - *self.range.start())
        };
        match event {
            Event::PointerMoved { pos } => {
                let inside = b.contains(*pos);
                let st = cx.state::<SliderState>();
                let dragging = st.dragging;
                if st.hovered != inside {
                    st.hovered = inside;
                    cx.request_redraw();
                }
                if inside || dragging {
                    cx.set_cursor(if dragging { CursorIcon::Grabbing } else { CursorIcon::Pointer });
                }
                if dragging {
                    let v = self.snap(value_at(pos.x));
                    if v != self.value {
                        self.value = v;
                        cx.emit((self.on_change)(v));
                    }
                }
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.state::<SliderState>().dragging = true;
                cx.request_focus();
                let v = self.snap(value_at(pos.x));
                if v != self.value {
                    self.value = v;
                    cx.emit((self.on_change)(v));
                }
                Status::Captured
            }
            Event::PointerReleased { .. } => {
                if std::mem::take(&mut cx.state::<SliderState>().dragging) {
                    cx.request_redraw();
                    if let Some(m) = self.on_release.clone() {
                        cx.emit(m);
                    }
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<SliderState>().hovered = false;
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let span = *self.range.end() - *self.range.start();
                let step = self.step.unwrap_or(span / 100.0);
                let v = match k.key {
                    Key::Left | Key::Down => self.value - step,
                    Key::Right | Key::Up => self.value + step,
                    Key::PageDown => self.value - span / 10.0,
                    Key::PageUp => self.value + span / 10.0,
                    Key::Home => *self.range.start(),
                    Key::End => *self.range.end(),
                    _ => return Status::Ignored,
                };
                let v = self.snap(v);
                if v != self.value {
                    self.value = v;
                    cx.emit((self.on_change)(v));
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
