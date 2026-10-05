use armature_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::style::lerp_paint;
use crate::ThemeCx;
use crate::anim::Anim;
use armature::controls::{byte_at, char_at, FieldAction, FieldLogic, FieldState};

use crate::core::{Cx, DrawCx, EventCx, Length, Limits, Padding, Widget};
use crate::event::{Event, Status};

/// What the painting keeps between frames; the caret and selection live
/// in the framework's [`FieldState`].
#[derive(Default)]
struct InputLook {
    focus: Anim,
}

/// A single-line text field. The application owns the text: edits arrive
/// through `on_input` and the new value comes back through the view.
pub struct TextInput<M> {
    value: String,
    placeholder: String,
    secure: bool,
    on_input: Option<Box<dyn Fn(String) -> M>>,
    on_submit: Option<M>,
    on_cancel: Option<M>,
    on_arrow: Option<Box<dyn Fn(i32) -> M>>,
    autofocus: bool,
    width: Length,
    padding: Padding,
    layout: Option<TextLayout>,
    placeholder_layout: Option<TextLayout>,
}

impl<M: Clone> TextInput<M> {
    pub fn new(placeholder: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            placeholder: placeholder.into(),
            secure: false,
            on_input: None,
            on_submit: None,
            on_cancel: None,
            on_arrow: None,
            autofocus: false,
            width: Length::Fill,
            padding: Padding::xy(14.0, 10.0),
            layout: None,
            placeholder_layout: None,
        }
    }

    pub fn on_input(mut self, f: impl Fn(String) -> M + 'static) -> Self {
        self.on_input = Some(Box::new(f));
        self
    }

    /// Sent when the user presses Enter.
    pub fn on_submit(mut self, m: M) -> Self {
        self.on_submit = Some(m);
        self
    }

    /// Sent when the user presses Escape.
    pub fn on_cancel(mut self, m: M) -> Self {
        self.on_cancel = Some(m);
        self
    }

    /// Up and Down send this, with -1 or 1, instead of moving the caret to
    /// the start or end. For a field that drives a list, such as a search box.
    pub fn on_arrow(mut self, f: impl Fn(i32) -> M + 'static) -> Self {
        self.on_arrow = Some(Box::new(f));
        self
    }

    /// Take keyboard focus, with all text selected, when the field first appears.
    pub fn autofocus(mut self, a: bool) -> Self {
        self.autofocus = a;
        self
    }

    /// Hide the characters, for passwords.
    pub fn secure(mut self, s: bool) -> Self {
        self.secure = s;
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    fn logic(&self) -> FieldLogic<'_> {
        FieldLogic { value: &self.value, secure: self.secure, arrows: self.on_arrow.is_some() }
    }

    fn shown(&self) -> String {
        self.logic().shown()
    }
}

/// Shorthand for [`TextInput::new`].
pub fn text_input<M: Clone>(placeholder: impl Into<String>, value: impl Into<String>) -> TextInput<M> {
    TextInput::new(placeholder, value)
}

impl<M: Clone + 'static> TextInput<M> {
    fn text_origin(&self, b: Rect, scroll: f32) -> Point {
        let h = self.layout.as_ref().map_or(0.0, |l| l.size().h);
        Point::new(b.x + self.padding.left - scroll, b.y + ((b.h - h) * 0.5).round())
    }

    fn hit(&self, b: Rect, scroll: f32, p: Point) -> usize {
        let Some(l) = &self.layout else { return 0 };
        let o = self.text_origin(b, scroll);
        let shown = self.shown();
        char_at(&shown, l.hit(Point::new(p.x - o.x, l.line_height() * 0.5)))
    }

    fn caret_x(&self, char_idx: usize) -> f32 {
        let Some(l) = &self.layout else { return 0.0 };
        l.caret(byte_at(&self.shown(), char_idx)).x
    }
}

impl<M: Clone + 'static> Widget<M> for TextInput<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = cx.theme().text(TextRole::Body).style();
        let shown = self.shown();
        self.layout = Some(cx.text().layout(&shown, &style, None));
        self.placeholder_layout = Some(cx.text().layout(&self.placeholder, &style, None));
        self.logic().sync(cx, self.autofocus);
        let h = style.size * style.line_height + self.padding.vertical();
        let l = limits.constrain(self.width, Length::Shrink);
        l.resolve(Size::new(if l.max.w.is_finite() { l.max.w } else { 220.0 }, h.round()))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let now = cx.now();
        let focused = cx.is_focused();
        let (cursor, anchor, blink) = {
            let st = cx.state::<FieldState>();
            (st.cursor, st.anchor, st.blink(now))
        };
        let (f, anim) = {
            let look = cx.state::<InputLook>();
            let f = look.focus.step(if focused { 1.0 } else { 0.0 }, now, theme.motion());
            (f, look.focus.is_animating())
        };
        if anim {
            cx.request_animation();
        }
        let radius = theme.control_radius() * 0.8;
        let mut focus_paint = theme.paint(Surface::Inset);
        focus_paint.border = Some((1.0, p.accent_text));
        let paint = lerp_paint(&theme.paint(Surface::Inset), &focus_paint, f);
        cx.scene.paint(b, radius, &paint);

        // Keep the caret in view.
        let inner_w = b.w - self.padding.horizontal();
        let caret = self.caret_x(cursor);
        let scroll = cx.state::<FieldState>().keep_caret_visible(caret, inner_w);

        let clip = Rect::new(b.x + self.padding.left - 1.0, b.y, inner_w + 3.0, b.h);
        cx.scene.push_clip(clip);
        let o = self.text_origin(b, scroll);
        let lh = self.layout.as_ref().map_or(0.0, |l| l.size().h);
        if focused && cursor != anchor {
            let (x0, x1) = (self.caret_x(cursor.min(anchor)), self.caret_x(cursor.max(anchor)));
            cx.scene.fill(Rect::new(o.x + x0, o.y, x1 - x0, lh), 3.0, p.accent.with_alpha(0.25), None);
        }
        if self.value.is_empty() {
            if let Some(l) = &self.placeholder_layout {
                cx.scene.text(l, o, p.faint);
            }
        } else if let Some(l) = &self.layout {
            cx.scene.text(l, o, p.text);
        }
        if focused {
            let (on, next) = blink;
            if on || theme.reduce_motion {
                cx.scene.fill(Rect::new((o.x + caret).round() - 0.75, o.y + 1.0, 1.5, lh - 2.0), 0.75, p.accent_text, None);
            }
            if !theme.reduce_motion {
                cx.request_redraw_after(next);
            }
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let scroll = cx.state::<FieldState>().scroll;
        let (status, action) = self.logic().event(cx, event, b, |p| self.hit(b, scroll, p));
        match action {
            Some(FieldAction::Edit(v)) => {
                if let Some(f) = &self.on_input {
                    let m = f(v.clone());
                    self.value = v;
                    cx.emit(m);
                }
            }
            Some(FieldAction::Submit) => {
                if let Some(m) = self.on_submit.clone() {
                    cx.emit(m);
                }
            }
            Some(FieldAction::Cancel) => {
                if let Some(m) = self.on_cancel.clone() {
                    cx.emit(m);
                }
            }
            Some(FieldAction::Arrow(by)) => {
                if let Some(f) = &self.on_arrow {
                    cx.emit(f(by));
                }
            }
            None => {}
        }
        status
    }
}
