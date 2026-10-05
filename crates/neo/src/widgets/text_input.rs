use std::time::{Duration, Instant};

use neo_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::style::lerp_paint;
use crate::anim::Anim;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Padding, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct InputState {
    /// Caret position as a char index into the value.
    cursor: usize,
    /// Other end of the selection, as a char index.
    anchor: usize,
    dragging: bool,
    hovered: bool,
    focus: Anim,
    scroll: f32,
    blink_origin: Option<Instant>,
    /// Set once the field has been laid out, so autofocus happens only once.
    mounted: bool,
    /// The value as of the last layout or edit. A different value arriving
    /// from the app means it was changed from outside, so the caret moves
    /// to the end.
    seen: Option<String>,
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

    fn shown(&self) -> String {
        if self.secure { "•".repeat(self.value.chars().count()) } else { self.value.clone() }
    }

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }
}

/// Shorthand for [`TextInput::new`].
pub fn text_input<M: Clone>(placeholder: impl Into<String>, value: impl Into<String>) -> TextInput<M> {
    TextInput::new(placeholder, value)
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

fn char_at(s: &str, byte_idx: usize) -> usize {
    s.char_indices().take_while(|(i, _)| *i < byte_idx).count()
}

/// Char index of the start of the word before `i`.
fn word_left(s: &str, i: usize) -> usize {
    let chars: Vec<char> = s.chars().collect();
    let mut j = i.min(chars.len());
    while j > 0 && chars[j - 1].is_whitespace() {
        j -= 1;
    }
    while j > 0 && !chars[j - 1].is_whitespace() {
        j -= 1;
    }
    j
}

/// Char index of the end of the word after `i`.
fn word_right(s: &str, i: usize) -> usize {
    let chars: Vec<char> = s.chars().collect();
    let mut j = i.min(chars.len());
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    while j < chars.len() && !chars[j].is_whitespace() {
        j += 1;
    }
    j
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

    /// Replaces the selection with `insert` and reports the edit.
    fn edit(&mut self, cx: &mut EventCx<M>, insert: &str) {
        let (cursor, anchor) = {
            let st = cx.state::<InputState>();
            (st.cursor, st.anchor)
        };
        let (lo, hi) = (cursor.min(anchor), cursor.max(anchor));
        let mut v = self.value.clone();
        let (blo, bhi) = (byte_at(&v, lo), byte_at(&v, hi));
        v.replace_range(blo..bhi, insert);
        let new_cursor = lo + insert.chars().count();
        {
            let st = cx.state::<InputState>();
            st.cursor = new_cursor;
            st.anchor = new_cursor;
            st.blink_origin = Some(Instant::now());
            st.seen = Some(v.clone());
        }
        if let Some(f) = &self.on_input {
            let m = f(v.clone());
            self.value = v;
            cx.emit(m);
        }
    }

    fn selected_text(&self, cursor: usize, anchor: usize) -> String {
        let (lo, hi) = (cursor.min(anchor), cursor.max(anchor));
        self.value.chars().skip(lo).take(hi - lo).collect()
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
        let n = self.char_count();
        let st = cx.state::<InputState>();
        if st.seen.as_ref().is_some_and(|s| *s != self.value) {
            st.cursor = n;
            st.anchor = n;
        }
        st.seen = Some(self.value.clone());
        st.cursor = st.cursor.min(n);
        st.anchor = st.anchor.min(n);
        if self.autofocus && !st.mounted {
            st.cursor = n;
            st.anchor = 0;
        }
        let first = !std::mem::replace(&mut st.mounted, true);
        if self.autofocus && first {
            cx.request_focus();
        }
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
        let (cursor, anchor, f, anim, blink_origin) = {
            let st = cx.state::<InputState>();
            let f = st.focus.step(if focused { 1.0 } else { 0.0 }, now, theme.motion());
            (st.cursor, st.anchor, f, st.focus.is_animating(), st.blink_origin)
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
        let scroll = {
            let st = cx.state::<InputState>();
            if caret - st.scroll > inner_w {
                st.scroll = caret - inner_w;
            } else if caret < st.scroll {
                st.scroll = caret;
            }
            st.scroll = st.scroll.max(0.0);
            st.scroll
        };

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
            let since = blink_origin.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
            let on = theme.reduce_motion || (since % 1.06) < 0.53;
            if on {
                cx.scene.fill(Rect::new((o.x + caret).round() - 0.75, o.y + 1.0, 1.5, lh - 2.0), 0.75, p.accent_text, None);
            }
            if !theme.reduce_motion {
                let next = 0.53 - (since % 0.53);
                cx.request_redraw_after(Duration::from_secs_f32(next.max(0.02)));
            }
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let scroll = cx.state::<InputState>().scroll;
        match event {
            Event::PointerMoved { pos } => {
                let inside = b.contains(*pos);
                let dragging = {
                    let st = cx.state::<InputState>();
                    st.hovered = inside;
                    st.dragging
                };
                if inside {
                    cx.set_cursor(CursorIcon::Text);
                }
                if dragging {
                    let c = self.hit(b, scroll, *pos);
                    cx.state::<InputState>().cursor = c;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => {
                if b.contains(*pos) {
                    let c = self.hit(b, scroll, *pos);
                    cx.request_focus();
                    let st = cx.state::<InputState>();
                    st.cursor = c;
                    st.anchor = c;
                    st.dragging = true;
                    st.blink_origin = Some(Instant::now());
                    Status::Captured
                } else {
                    cx.release_focus();
                    Status::Ignored
                }
            }
            Event::PointerReleased { .. } => {
                cx.state::<InputState>().dragging = false;
                Status::Ignored
            }
            Event::Ime(t) if cx.is_focused() => {
                self.edit(cx, t);
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let n = self.char_count();
                let (cursor, anchor) = {
                    let st = cx.state::<InputState>();
                    (st.cursor, st.anchor)
                };
                let shift = k.modifiers.shift;
                let cmd = k.modifiers.command();
                let word = if cfg!(target_os = "macos") { k.modifiers.alt } else { k.modifiers.ctrl };
                let move_to = |cx: &mut EventCx<M>, c: usize| {
                    let st = cx.state::<InputState>();
                    st.cursor = c;
                    if !shift {
                        st.anchor = c;
                    }
                    st.blink_origin = Some(Instant::now());
                    cx.request_redraw();
                };
                match &k.key {
                    Key::Left => {
                        let c = if cmd { 0 } else if word { word_left(&self.value, cursor) } else if cursor != anchor && !shift { cursor.min(anchor) } else { cursor.saturating_sub(1) };
                        move_to(cx, c);
                    }
                    Key::Right => {
                        let c = if cmd { n } else if word { word_right(&self.value, cursor) } else if cursor != anchor && !shift { cursor.max(anchor) } else { (cursor + 1).min(n) };
                        move_to(cx, c);
                    }
                    Key::Home | Key::Up => move_to(cx, 0),
                    Key::End | Key::Down => move_to(cx, n),
                    Key::Backspace => {
                        if cursor == anchor {
                            if cursor == 0 {
                                return Status::Captured;
                            }
                            let from = if word { word_left(&self.value, cursor) } else { cursor - 1 };
                            cx.state::<InputState>().anchor = from;
                        }
                        self.edit(cx, "");
                    }
                    Key::Delete => {
                        if cursor == anchor {
                            if cursor >= n {
                                return Status::Captured;
                            }
                            let to = if word { word_right(&self.value, cursor) } else { cursor + 1 };
                            cx.state::<InputState>().anchor = to;
                        }
                        self.edit(cx, "");
                    }
                    Key::Enter => {
                        if let Some(m) = self.on_submit.clone() {
                            cx.emit(m);
                        }
                    }
                    Key::Escape => {
                        cx.release_focus();
                        cx.request_redraw();
                        if let Some(m) = self.on_cancel.clone() {
                            cx.emit(m);
                        }
                    }
                    Key::Tab => return Status::Ignored,
                    Key::Character(c) if cmd => match c.as_str() {
                        "a" => {
                            let st = cx.state::<InputState>();
                            st.anchor = 0;
                            st.cursor = n;
                            cx.request_redraw();
                        }
                        "c" | "x" if cursor != anchor && !self.secure => {
                            let t = self.selected_text(cursor, anchor);
                            cx.copy(t);
                            if c == "x" {
                                self.edit(cx, "");
                            }
                        }
                        "v" => {
                            if let Some(t) = cx.clipboard().map(|t| t.replace(['\n', '\r'], " ")) {
                                self.edit(cx, &t);
                            }
                        }
                        _ => return Status::Ignored,
                    },
                    _ => {
                        let text: Option<String> = k.text.clone().filter(|t| !t.chars().any(char::is_control) && !k.modifiers.ctrl && !k.modifiers.logo);
                        match text {
                            Some(t) => self.edit(cx, &t),
                            None => return Status::Ignored,
                        }
                    }
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_motion() {
        let s = "hello brave  world";
        assert_eq!(word_left(s, s.chars().count()), 13);
        assert_eq!(word_left(s, 13), 6);
        assert_eq!(word_right(s, 0), 5);
        assert_eq!(word_right(s, 5), 11);
    }

    #[test]
    fn char_byte_mapping() {
        let s = "añb";
        assert_eq!(byte_at(s, 2), 3);
        assert_eq!(char_at(s, 3), 2);
        assert_eq!(byte_at(s, 9), s.len());
    }
}
