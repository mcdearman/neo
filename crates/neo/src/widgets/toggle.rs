use armature_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Color, Surface, TextRole};

use crate::{FocusRing, ThemeCx};
use crate::anim::Anim;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct SwitchState {
    pressed: bool,
    pos: Option<Anim>,
}

/// An on/off switch.
pub struct Toggle<M> {
    on: bool,
    on_toggle: Option<Box<dyn Fn(bool) -> M>>,
}

impl<M> Toggle<M> {
    pub fn new(on: bool) -> Self {
        Self { on, on_toggle: None }
    }

    /// Called with the new value when the user flips the switch.
    pub fn on_toggle(mut self, f: impl Fn(bool) -> M + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

/// Shorthand for `Toggle::new(on).on_toggle(f)`.
pub fn toggle<M>(on: bool, f: impl Fn(bool) -> M + 'static) -> Toggle<M> {
    Toggle::new(on).on_toggle(f)
}

const SWITCH: Size = Size::new(50.0, 28.0);

impl<M: 'static> Widget<M> for Toggle<M> {
    fn focusable(&self) -> bool {
        self.on_toggle.is_some()
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        limits.resolve(SWITCH)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let now = cx.now();
        let target = if self.on { 1.0 } else { 0.0 };
        let (t, animating) = {
            let st = cx.state::<SwitchState>();
            let a = st.pos.get_or_insert(Anim::new(target));
            a.step(target, now, theme.motion() * 1.3);
            (a.eased(), a.is_animating())
        };
        if animating {
            cx.request_animation();
        }
        let track = Rect::new(b.x, b.y + (b.h - SWITCH.h) * 0.5, SWITCH.w, SWITCH.h);
        let r = track.h * 0.5;
        let mut paint = theme.paint(Surface::Inset);
        paint.fill = paint.fill.mix(p.accent, t);
        if let Some((w, c)) = paint.border {
            paint.border = Some((w, c.mix(p.accent, t)));
        }
        cx.scene.paint(track, r, &paint);
        cx.focus_ring(track, r);

        let d = track.h - 8.0;
        let x = track.x + 4.0 + (track.w - d - 8.0) * t;
        let knob = Rect::new(x, track.y + 4.0, d, d);
        let mut kp = theme.paint(Surface::Raised);
        kp.fill = kp.fill.mix(Color::WHITE, t);
        kp.border = kp.border.map(|(w, c)| (w, c.mix(p.accent, t)));
        cx.scene.paint(knob, d * 0.5, &kp);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let Some(f) = &self.on_toggle else { return Status::Ignored };
        match event {
            Event::PointerMoved { pos } if b.contains(*pos) => {
                cx.set_cursor(CursorIcon::Pointer);
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.state::<SwitchState>().pressed = true;
                Status::Captured
            }
            Event::PointerReleased { pos, .. } => {
                let st = cx.state::<SwitchState>();
                if std::mem::take(&mut st.pressed) && b.contains(*pos) {
                    let m = f(!self.on);
                    cx.emit(m);
                }
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() && matches!(k.key, Key::Space | Key::Enter) => {
                let m = f(!self.on);
                cx.emit(m);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

#[derive(Default)]
struct CheckState {
    pressed: bool,
}

/// A checkbox with an optional label.
pub struct Checkbox<M> {
    checked: bool,
    label: Option<String>,
    on_toggle: Option<Box<dyn Fn(bool) -> M>>,
    layout: Option<TextLayout>,
}

impl<M> Checkbox<M> {
    pub fn new(checked: bool) -> Self {
        Self { checked, label: None, on_toggle: None, layout: None }
    }

    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }

    pub fn on_toggle(mut self, f: impl Fn(bool) -> M + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

/// Shorthand for `Checkbox::new(checked).label(label).on_toggle(f)`.
pub fn checkbox<M>(label: impl Into<String>, checked: bool, f: impl Fn(bool) -> M + 'static) -> Checkbox<M> {
    Checkbox::new(checked).label(label).on_toggle(f)
}

const BOX: f32 = 20.0;

impl<M: 'static> Widget<M> for Checkbox<M> {
    fn focusable(&self) -> bool {
        self.on_toggle.is_some()
    }

    fn width(&self) -> Length {
        Length::Shrink
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let spec = cx.theme().text(TextRole::Body);
        self.layout = self.label.as_ref().map(|l| cx.text().layout(l, &spec.style(), None));
        let tw = self.layout.as_ref().map_or(0.0, |l| l.size().w + 10.0);
        let th = self.layout.as_ref().map_or(0.0, |l| l.size().h);
        limits.resolve(Size::new(BOX + tw, BOX.max(th)))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let r = Rect::new(b.x, b.y + (b.h - BOX) * 0.5, BOX, BOX);
        let radius = (theme.small_radius() * 0.6).max(4.0);
        if self.checked {
            cx.scene.paint(r, radius, &theme.paint(Surface::Accent));
            // Check mark.
            let a = Point::new(r.x + 5.0, r.y + 10.5);
            let m = Point::new(r.x + 8.5, r.y + 14.0);
            let e = Point::new(r.x + 15.0, r.y + 6.5);
            cx.scene.polyline(&[a, m, e], 2.2, p.on_accent);
        } else {
            cx.scene.paint(r, radius, &theme.paint(Surface::Inset));
        }
        cx.focus_ring(r, radius);
        if let Some(l) = &self.layout {
            let color = cx.content_color();
            cx.scene.text(l, Point::new(r.right() + 10.0, b.y + (b.h - l.size().h) * 0.5), color);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let Some(f) = &self.on_toggle else { return Status::Ignored };
        match event {
            Event::PointerMoved { pos } if b.contains(*pos) => {
                cx.set_cursor(CursorIcon::Pointer);
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.state::<CheckState>().pressed = true;
                Status::Captured
            }
            Event::PointerReleased { pos, .. } => {
                if std::mem::take(&mut cx.state::<CheckState>().pressed) && b.contains(*pos) {
                    let m = f(!self.checked);
                    cx.emit(m);
                }
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() && matches!(k.key, Key::Space) => {
                let m = f(!self.checked);
                cx.emit(m);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
