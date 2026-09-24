use std::time::{Duration, Instant};

use neo_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::Color;

use super::document::{text_between, Action, Document, Motion, Pos};
use super::highlight::{highlight, Language, SyntaxColors};
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct EditorState {
    scroll: Point,
    last_cursor: Option<(Pos, usize)>,
    dragging: bool,
    last_click: Option<(Instant, Pos)>,
    blink_origin: Option<Instant>,
    view: Size,
}

/// A multi-line code editor with line numbers and syntax highlighting.
///
/// The application owns the [`Document`] and applies the [`Action`]s this
/// widget sends, so it can track changes, save, or refuse edits.
pub struct TextEditor<M> {
    lines: Vec<String>,
    cursor: Pos,
    anchor: Pos,
    revision: u64,
    language: Language,
    on_action: Option<Box<dyn Fn(Action) -> M>>,
    width: Length,
    height: Length,
    font_size: f32,
    // Layout results.
    line_h: f32,
    char_w: f32,
    gutter_w: f32,
    first: usize,
    rows: Vec<TextLayout>,
    numbers: Vec<TextLayout>,
    longest: f32,
}

impl<M> TextEditor<M> {
    pub fn new(doc: &Document) -> Self {
        Self {
            lines: doc.lines().to_vec(),
            cursor: doc.cursor(),
            anchor: doc.anchor(),
            revision: doc.revision(),
            language: Language::Plain,
            on_action: None,
            width: Length::Fill,
            height: Length::Fill,
            font_size: 13.5,
            line_h: 0.0,
            char_w: 0.0,
            gutter_w: 0.0,
            first: 0,
            rows: vec![],
            numbers: vec![],
            longest: 0.0,
        }
    }

    pub fn language(mut self, l: Language) -> Self {
        self.language = l;
        self
    }

    /// Called for every edit and cursor change. Without it the editor is read-only.
    pub fn on_action(mut self, f: impl Fn(Action) -> M + 'static) -> Self {
        self.on_action = Some(Box::new(f));
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }

    /// Font size before the user's text scale.
    pub fn font_size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }

    fn selection(&self) -> Option<(Pos, Pos)> {
        (self.cursor != self.anchor).then(|| (self.cursor.min(self.anchor), self.cursor.max(self.anchor)))
    }

    fn row(&self, line: usize) -> Option<&TextLayout> {
        line.checked_sub(self.first).and_then(|i| self.rows.get(i))
    }
}

/// Shorthand for [`TextEditor::new`].
pub fn text_editor<M>(doc: &Document) -> TextEditor<M> {
    TextEditor::new(doc)
}

const PAD_Y: f32 = 10.0;
const PAD_X: f32 = 14.0;
const GUTTER_PAD: f32 = 14.0;

impl<M: 'static> TextEditor<M> {
    fn style(&self, cx: &Cx) -> TextStyle {
        TextStyle { size: self.font_size * cx.theme().text_scale, weight: 400, family: FontFamily::Mono, line_height: 1.6, letter_spacing: 0.0 }
    }

    fn text_x(&self, b: Rect, scroll: Point) -> f32 {
        b.x + self.gutter_w + PAD_X - scroll.x
    }

    fn line_y(&self, b: Rect, scroll: Point, line: usize) -> f32 {
        b.y + PAD_Y + line as f32 * self.line_h - scroll.y
    }

    fn caret_x(&self, line: usize, col: usize) -> f32 {
        match self.row(line) {
            Some(l) if col > 0 => l.caret(col).x,
            _ => 0.0,
        }
    }

    /// Document position under window point `p`, clamped to the text.
    fn pos_at(&self, b: Rect, scroll: Point, p: Point) -> Pos {
        let rel = p.y - b.y - PAD_Y + scroll.y;
        let line = if rel < 0.0 { 0 } else { ((rel / self.line_h) as usize).min(self.lines.len() - 1) };
        let x = p.x - self.text_x(b, scroll);
        let col = if x <= 0.0 {
            0
        } else if let Some(l) = self.row(line) {
            l.hit(Point::new(x, self.line_h * 0.5)).min(self.lines[line].len())
        } else {
            // Off-screen line (dragging past the edge): estimate from the grid.
            let chars = (x / self.char_w.max(1.0)).round() as usize;
            self.lines[line].char_indices().nth(chars).map_or(self.lines[line].len(), |(i, _)| i)
        };
        Pos::new(line, col)
    }

    fn page(&self, view_h: f32) -> usize {
        ((view_h - PAD_Y * 2.0) / self.line_h.max(1.0)).floor().max(1.0) as usize
    }
}

impl<M: 'static> Widget<M> for TextEditor<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn height(&self) -> Length {
        self.height
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let l = limits.constrain(self.width, self.height);
        let size = l.resolve(Size::new(if l.max.w.is_finite() { l.max.w } else { 600.0 }, if l.max.h.is_finite() { l.max.h } else { 400.0 }));
        let style = self.style(cx);
        let digit = cx.text().layout("0", &style, None);
        self.char_w = digit.size().w.max(1.0);
        self.line_h = digit.size().h.max(1.0);
        let digits = self.lines.len().to_string().len().max(3);
        self.gutter_w = (digits as f32 * self.char_w + GUTTER_PAD * 2.0).round();
        self.longest = self.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * self.char_w;

        let theme = *cx.theme();
        let colors = SyntaxColors::new(&theme.palette(), theme.scheme, theme.accent);
        let text_w = size.w - self.gutter_w - PAD_X;
        let total_h = self.lines.len() as f32 * self.line_h + PAD_Y * 2.0;
        let max_y = if total_h > size.h { total_h - size.h * 0.5 } else { 0.0 };
        let max_x = (self.longest + PAD_X * 2.0 + self.char_w - text_w).max(0.0);

        // Keep the cursor in view when it moved since the last layout.
        let cursor_line_layout = {
            let mut in_comment = false;
            if self.language == Language::Rust {
                for l in &self.lines[..self.cursor.line] {
                    highlight(self.language, l, &mut in_comment);
                }
            }
            let line = &self.lines[self.cursor.line];
            cx.text().layout(line, &style, None)
        };
        let (line_h, cursor, revision) = (self.line_h, self.cursor, self.revision);
        let caret = if cursor.col > 0 { cursor_line_layout.caret(cursor.col).x } else { 0.0 };
        let st = cx.state::<EditorState>();
        st.view = size;
        if st.last_cursor != Some((cursor, revision as usize)) {
            let moved = st.last_cursor.is_some_and(|(p, _)| p != cursor) || st.last_cursor.is_none_or(|(_, r)| r != revision as usize);
            st.last_cursor = Some((cursor, revision as usize));
            if moved {
                let top = cursor.line as f32 * line_h;
                if top < st.scroll.y {
                    st.scroll.y = top;
                } else if top + line_h + PAD_Y * 2.0 > st.scroll.y + size.h {
                    st.scroll.y = top + line_h + PAD_Y * 2.0 - size.h;
                }
                let margin = self.char_w * 4.0;
                if caret < st.scroll.x + margin {
                    st.scroll.x = (caret - margin).max(0.0);
                } else if caret > st.scroll.x + text_w - margin - PAD_X {
                    st.scroll.x = caret - text_w + margin + PAD_X;
                }
            }
        }
        st.scroll.y = st.scroll.y.clamp(0.0, max_y);
        st.scroll.x = st.scroll.x.clamp(0.0, max_x);
        let scroll = st.scroll;

        // Lay out only the visible lines.
        self.first = ((scroll.y - PAD_Y) / self.line_h).floor().max(0.0) as usize;
        let count = (size.h / self.line_h).ceil() as usize + 2;
        let last = (self.first + count).min(self.lines.len());
        self.first = self.first.min(last);
        let mut in_comment = false;
        if self.language == Language::Rust {
            for l in &self.lines[..self.first] {
                highlight(self.language, l, &mut in_comment);
            }
        }
        self.rows.clear();
        self.numbers.clear();
        let num_style = TextStyle { size: style.size * 0.92, ..style };
        for i in self.first..last {
            let line = &self.lines[i];
            let spans: Vec<(&str, Option<Color>)> = highlight(self.language, line, &mut in_comment).into_iter().map(|(a, b, k)| (&line[a..b], colors.color(k))).collect();
            let layout = if spans.len() <= 1 && spans.first().is_none_or(|s| s.1.is_none()) {
                cx.text().layout(line, &style, None)
            } else {
                cx.text().layout_spans(&spans, &style, None)
            };
            self.rows.push(layout);
            self.numbers.push(cx.text().layout(&(i + 1).to_string(), &num_style, None));
        }
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let now = cx.now();
        let focused = cx.is_focused();
        let (scroll, blink_origin) = {
            let st = cx.state::<EditorState>();
            (st.scroll, st.blink_origin)
        };
        let text_area = Rect::new(b.x + self.gutter_w, b.y, (b.w - self.gutter_w).max(0.0), b.h);
        let sel = self.selection();

        cx.scene.push_clip(b);
        // Current line.
        if sel.is_none() {
            let y = self.line_y(b, scroll, self.cursor.line);
            cx.scene.fill(Rect::new(b.x, y, b.w, self.line_h), 0.0, p.accent.with_alpha(if focused { 0.08 } else { 0.04 }), None);
        }

        cx.scene.push_clip(text_area);
        let tx = self.text_x(b, scroll);
        if let Some((s, e)) = sel {
            let sel_color = p.accent.with_alpha(if focused { 0.28 } else { 0.16 });
            for line in s.line.max(self.first)..=e.line.min(self.first + self.rows.len().saturating_sub(1)) {
                let from = if line == s.line { self.caret_x(line, s.col) } else { 0.0 };
                let to = if line == e.line { self.caret_x(line, e.col) } else { self.caret_x(line, self.lines[line].len()) + self.char_w * 0.6 };
                if to > from {
                    let y = self.line_y(b, scroll, line);
                    cx.scene.fill(Rect::new(tx + from, y, to - from, self.line_h), 3.0, sel_color, None);
                }
            }
        }
        let default = p.text;
        for (i, layout) in self.rows.iter().enumerate() {
            let y = self.line_y(b, scroll, self.first + i);
            cx.scene.text(layout, Point::new(tx.round(), (y + (self.line_h - layout.size().h) * 0.5).round()), default);
        }
        if focused {
            let since = blink_origin.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
            if theme.reduce_motion || since % 1.06 < 0.53 {
                let x = tx + self.caret_x(self.cursor.line, self.cursor.col);
                let y = self.line_y(b, scroll, self.cursor.line);
                cx.scene.fill(Rect::new(x.round() - 1.0, y + 2.0, 2.0, self.line_h - 4.0), 1.0, p.accent_text, None);
            }
            if !theme.reduce_motion {
                cx.request_redraw_after(Duration::from_secs_f32((0.53 - since % 0.53).max(0.02)));
            }
        }
        cx.scene.pop_clip();

        // Gutter: separator and line numbers.
        let sep = Rect::new(b.x + self.gutter_w - 1.0, b.y, 1.0, b.h);
        cx.scene.fill(sep, 0.0, if theme.is_soft() { p.shadow_dark.with_alpha(0.6) } else { p.line.with_alpha(0.7) }, None);
        for (i, n) in self.numbers.iter().enumerate() {
            let line = self.first + i;
            let y = self.line_y(b, scroll, line);
            let current = sel.map_or(line == self.cursor.line, |(s, e)| line >= s.line && line <= e.line);
            let color = if current { p.text } else { p.faint };
            let x = b.x + self.gutter_w - GUTTER_PAD - n.size().w;
            cx.scene.text(n, Point::new(x.round(), (y + (self.line_h - n.size().h) * 0.5).round()), color);
        }

        // Scrollbars.
        let total_h = self.lines.len() as f32 * self.line_h + PAD_Y * 2.0;
        if total_h > b.h {
            let content = total_h + b.h * 0.5;
            let h = (b.h * b.h / content).max(28.0);
            let y = b.y + (b.h - h) * (scroll.y / (content - b.h).max(1.0));
            cx.scene.fill(Rect::new(b.right() - 9.0, y, 6.0, h), 3.0, p.faint.with_alpha(0.4), None);
        }
        let text_w = text_area.w - PAD_X;
        let content_w = self.longest + PAD_X * 2.0 + self.char_w;
        if content_w > text_w {
            let w = (text_area.w * text_w / content_w).max(28.0);
            let x = text_area.x + (text_area.w - w) * (scroll.x / (content_w - text_w).max(1.0));
            cx.scene.fill(Rect::new(x, b.bottom() - 9.0, w, 6.0), 3.0, p.faint.with_alpha(0.4), None);
        }
        cx.scene.pop_clip();
        cx.focus_ring(b, theme.control_radius());
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let Some(f) = self.on_action.as_ref() else { return Status::Ignored };
        let scroll = cx.state::<EditorState>().scroll;
        let send = |cx: &mut EventCx<M>, a: Action| {
            cx.state::<EditorState>().blink_origin = Some(Instant::now());
            cx.emit(f(a));
        };
        match event {
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let st = cx.state::<EditorState>();
                st.scroll.x += delta.x;
                st.scroll.y += delta.y;
                cx.request_layout();
                Status::Captured
            }
            Event::PointerMoved { pos } => {
                let dragging = cx.state::<EditorState>().dragging;
                if b.contains(*pos) {
                    cx.set_cursor(if pos.x > b.x + self.gutter_w { CursorIcon::Text } else { CursorIcon::Default });
                }
                if dragging {
                    // Scroll when dragging past the top or bottom edge.
                    if pos.y < b.y || pos.y > b.bottom() {
                        let d = if pos.y < b.y { pos.y - b.y } else { pos.y - b.bottom() };
                        cx.state::<EditorState>().scroll.y += d.clamp(-self.line_h * 2.0, self.line_h * 2.0);
                        cx.request_layout();
                    }
                    let p = self.pos_at(b, scroll, *pos);
                    if p != self.cursor {
                        self.cursor = p;
                        send(cx, Action::Drag(p));
                    }
                }
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => {
                if !b.contains(*pos) {
                    return Status::Ignored;
                }
                cx.request_focus();
                let p = self.pos_at(b, scroll, *pos);
                let now = Instant::now();
                let shift = cx.modifiers().shift;
                let double = {
                    let st = cx.state::<EditorState>();
                    let d = st.last_click.is_some_and(|(t, q)| now.duration_since(t) < Duration::from_millis(400) && q == p);
                    st.last_click = Some((now, p));
                    st.dragging = !d;
                    d
                };
                send(cx, if double { Action::SelectWord(p) } else { Action::Click { pos: p, select: shift } });
                Status::Captured
            }
            Event::PointerReleased { .. } => {
                cx.state::<EditorState>().dragging = false;
                Status::Ignored
            }
            Event::Ime(t) if cx.is_focused() => {
                send(cx, Action::Insert(t.clone()));
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let m = k.modifiers;
                let cmd = m.command();
                let word = if cfg!(target_os = "macos") { m.alt } else { m.ctrl };
                let select = m.shift;
                let page = self.page(cx.state::<EditorState>().view.h);
                let motion = |mo: Motion| Action::Move { motion: mo, select };
                let action = match &k.key {
                    Key::Left if cmd => motion(Motion::LineStart),
                    Key::Right if cmd => motion(Motion::LineEnd),
                    Key::Up if cmd => motion(Motion::DocStart),
                    Key::Down if cmd => motion(Motion::DocEnd),
                    Key::Left if word => motion(Motion::WordLeft),
                    Key::Right if word => motion(Motion::WordRight),
                    Key::Left => motion(Motion::Left),
                    Key::Right => motion(Motion::Right),
                    Key::Up => motion(Motion::Up),
                    Key::Down => motion(Motion::Down),
                    Key::Home => motion(if cmd { Motion::DocStart } else { Motion::LineStart }),
                    Key::End => motion(if cmd { Motion::DocEnd } else { Motion::LineEnd }),
                    Key::PageUp => motion(Motion::PageUp(page)),
                    Key::PageDown => motion(Motion::PageDown(page)),
                    Key::Enter => Action::Enter,
                    Key::Tab if m.shift => Action::Outdent,
                    Key::Tab => Action::Indent,
                    Key::Escape if self.selection().is_some() => Action::Collapse,
                    Key::Escape => {
                        cx.release_focus();
                        cx.request_redraw();
                        return Status::Captured;
                    }
                    Key::Backspace | Key::Delete => {
                        if self.selection().is_none() && (word || cmd) {
                            let mo = match (&k.key, cmd) {
                                (Key::Backspace, true) => Motion::LineStart,
                                (Key::Backspace, false) => Motion::WordLeft,
                                (_, true) => Motion::LineEnd,
                                _ => Motion::WordRight,
                            };
                            send(cx, Action::Move { motion: mo, select: true });
                        }
                        if k.key == Key::Backspace { Action::Backspace } else { Action::Delete }
                    }
                    Key::Character(c) if cmd => match c.as_str() {
                        "a" => Action::SelectAll,
                        "z" if m.shift => Action::Redo,
                        "z" => Action::Undo,
                        "y" => Action::Redo,
                        "c" | "x" => {
                            let Some((a, e)) = self.selection() else { return Status::Captured };
                            cx.copy(text_between(&self.lines, a, e));
                            if c == "c" {
                                return Status::Captured;
                            }
                            Action::Backspace
                        }
                        "v" => match cx.clipboard() {
                            Some(t) => Action::Insert(t.to_owned()),
                            None => return Status::Captured,
                        },
                        _ => return Status::Ignored,
                    },
                    _ => match k.text.as_deref().filter(|t| !t.chars().any(char::is_control) && !m.ctrl && !m.logo) {
                        Some(t) => Action::Insert(t.to_owned()),
                        None => return Status::Ignored,
                    },
                };
                send(cx, action);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
