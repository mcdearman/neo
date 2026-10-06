use std::time::{Duration, Instant};

use armature_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Color, Surface};

use armature::document::{text_between, Action, ClipboardNeed, Document, ExtraSelection, Motion, Pos, Scroll, VimView};
use super::highlight::{highlight, overlay, Kind, Language, SyntaxColors};
use super::style::Tone;
use crate::{FocusRing, ThemeCx};
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct EditorState {
    scroll: Point,
    last_cursor: Option<(Pos, usize)>,
    dragging: bool,
    /// The word the mouse is over, by where it starts.
    pointed: Option<Pos>,
    last_click: Option<(Instant, Pos)>,
    blink_origin: Option<Instant>,
    view: Size,
    /// Serials of the last Vim clipboard write and scroll request handled.
    clip_serial: u64,
    scroll_serial: u64,
}

/// A multi-line code editor with line numbers and syntax highlighting.
///
/// The application owns the [`Document`] and applies the [`Action`]s this
/// widget sends, so it can track changes, save, or refuse edits.
pub struct TextEditor<M> {
    lines: Vec<String>,
    cursor: Pos,
    selection: Option<(Pos, Pos, bool)>,
    /// Selections besides the main one, which Helix keys can make.
    extras: Vec<ExtraSelection>,
    block_caret: bool,
    marks: Vec<EditorMark>,
    /// Sorted by line.
    tokens: std::rc::Rc<[EditorToken]>,
    popup: Option<EditorPopup>,
    popup_at: Option<Pos>,
    on_point: Option<Box<dyn Fn(Option<Pos>) -> M>>,
    on_popup_key: Option<Box<dyn Fn(PopupKey) -> M>>,
    /// The popup's rows as laid out, and the first list item among them.
    popup_rows: Vec<(TextLayout, Option<TextLayout>)>,
    popup_first: usize,
    vim: Option<VimView>,
    revision: u64,
    /// First visible line and number of whole visible lines, for Vim.
    viewport: (usize, usize),
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
            selection: doc.display_selection(),
            extras: doc.extra_selections(),
            block_caret: doc.block_caret(),
            marks: vec![],
            tokens: std::rc::Rc::from([]),
            popup: None,
            popup_at: None,
            on_point: None,
            on_popup_key: None,
            popup_rows: vec![],
            popup_first: 0,
            vim: doc.vim_view(),
            revision: doc.revision(),
            viewport: (0, 0),
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

    /// Underlines stretches of the text.
    pub fn marks(mut self, marks: Vec<EditorMark>) -> Self {
        self.marks = marks;
        self
    }

    /// Colours the text by these, which must be sorted by line, wherever
    /// they reach; the built-in highlighter colours the rest.
    pub fn tokens(mut self, tokens: std::rc::Rc<[EditorToken]>) -> Self {
        self.tokens = tokens;
        self
    }

    /// Shows a popup at the caret.
    pub fn popup(mut self, popup: Option<EditorPopup>) -> Self {
        self.popup = popup;
        self
    }

    /// Puts the popup at this place in the text rather than at the caret.
    pub fn popup_at(mut self, at: Option<Pos>) -> Self {
        self.popup_at = at;
        self
    }

    /// Called when the mouse comes to rest over a different word, with
    /// where that word starts, and with `None` when it is over no word.
    pub fn on_point(mut self, f: impl Fn(Option<Pos>) -> M + 'static) -> Self {
        self.on_point = Some(Box::new(f));
        self
    }

    /// Called with the keys that work the popup while one is showing:
    /// Escape for any popup, and the arrows, Enter and Tab for a list.
    /// Without it, those keys edit as usual.
    pub fn on_popup_key(mut self, f: impl Fn(PopupKey) -> M + 'static) -> Self {
        self.on_popup_key = Some(Box::new(f));
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
        self.selection.map(|(a, b, _)| (a, b))
    }

    /// The selected text as the document would copy it.
    fn selected_text(&self) -> Option<String> {
        if let Some(block) = self.vim.as_ref().and_then(|v| v.block) {
            return Some(block.text(&self.lines));
        }
        let (a, b, linewise) = self.selection?;
        Some(if linewise { self.lines[a.line..=b.line].join("\n") + "\n" } else { text_between(&self.lines, a, b) })
    }

    fn row(&self, line: usize) -> Option<&TextLayout> {
        line.checked_sub(self.first).and_then(|i| self.rows.get(i))
    }
}

/// A stretch of text to underline, such as a problem a language server found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorMark {
    pub from: Pos,
    pub to: Pos,
    pub tone: Tone,
}

/// A stretch of one line and what it is, from a source that understands
/// the code better than the built-in highlighter, such as a language
/// server. `from` and `to` are byte offsets into the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorToken {
    pub line: usize,
    pub from: usize,
    pub to: usize,
    pub kind: Kind,
}

/// Something to show at the caret.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorPopup {
    /// A note about what is under the caret.
    Text(String),
    /// Choices, each with an optional detail in a quieter colour, and which
    /// one is picked.
    List { items: Vec<(String, Option<String>)>, selected: usize },
}

/// A key the editor hands over while a popup is showing, instead of
/// editing with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKey {
    Up,
    Down,
    /// Enter or Tab on a list.
    Accept,
    /// Escape.
    Dismiss,
}

/// Rows of a list popup shown at once, and lines of a text one.
const POPUP_ROWS: usize = 8;
const POPUP_LINES: usize = 16;

/// Ctrl keys that the modal keymaps (Vim and Helix) handle; the rest go to
/// the application.
fn vim_ctrl(key: &Key) -> bool {
    matches!(key, Key::Character(c) if matches!(c.as_str(), "r" | "d" | "u" | "f" | "b" | "n" | "p" | "w" | "h" | "[" | "c" | "e" | "y" | "v"))
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

    /// Where the word under window point `p` starts, if there is one: not
    /// past the end of a line, in the margin, or between words.
    fn word_at(&self, b: Rect, scroll: Point, p: Point) -> Option<Pos> {
        if !b.contains(p) || p.x <= b.x + self.gutter_w {
            return None;
        }
        let rel = p.y - b.y - PAD_Y + scroll.y;
        let line = (rel >= 0.0).then(|| (rel / self.line_h) as usize).filter(|l| *l < self.lines.len())?;
        let (row, text) = (self.row(line)?, &self.lines[line]);
        let x = p.x - self.text_x(b, scroll);
        if x < 0.0 || x >= row.size().w {
            return None;
        }
        // The nearest gap between characters, then the character the
        // point is actually on, which may be the one before that gap.
        let mut at = row.hit(Point::new(x, self.line_h * 0.5)).min(text.len());
        if at > 0 && row.caret(at).x > x {
            at = text[..at].char_indices().next_back().map_or(0, |(i, _)| i);
        }
        let word = |c: char| c.is_alphanumeric() || c == '_';
        text[at..].chars().next().filter(|c| word(*c))?;
        let start = text[..at].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(at, |(i, _)| i);
        Some(Pos::new(line, start))
    }

    /// Tells the app which word the mouse is over, when that changes.
    fn point(&self, cx: &mut EventCx<M>, word: Option<Pos>) {
        let Some(on_point) = &self.on_point else { return };
        let st = cx.state::<EditorState>();
        if st.pointed != word {
            st.pointed = word;
            cx.emit(on_point(word));
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
        self.popup_rows.clear();
        self.popup_first = 0;
        match &self.popup {
            Some(EditorPopup::Text(text)) => {
                for line in text.lines().take(POPUP_LINES) {
                    let line: String = line.chars().take(100).collect();
                    self.popup_rows.push((cx.text().layout(&line, &style, None), None));
                }
            }
            Some(EditorPopup::List { items, selected }) => {
                // The rows shown slide to keep the picked one in view.
                self.popup_first = selected.saturating_sub(POPUP_ROWS - 1).min(items.len().saturating_sub(POPUP_ROWS));
                for (label, detail) in items.iter().skip(self.popup_first).take(POPUP_ROWS) {
                    let detail = detail.as_ref().map(|d| cx.text().layout(&d.chars().take(60).collect::<String>(), &style, None));
                    self.popup_rows.push((cx.text().layout(label, &style, None), detail));
                }
            }
            None => {}
        }
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
            if self.language.has_block_comments() {
                for l in &self.lines[..self.cursor.line] {
                    highlight(self.language, l, &mut in_comment);
                }
            }
            let line = &self.lines[self.cursor.line];
            cx.text().layout(line, &style, None)
        };
        let (line_h, cursor, revision) = (self.line_h, self.cursor, self.revision);
        let caret = if cursor.col > 0 { cursor_line_layout.caret(cursor.col).x } else { 0.0 };
        // Vim: copy yanks to the system clipboard once each.
        if let Some((serial, text)) = self.vim.as_ref().and_then(|v| v.clipboard.clone())
            && cx.state::<EditorState>().clip_serial != serial {
                cx.state::<EditorState>().clip_serial = serial;
                cx.copy(text);
            }
        let scroll_request = self.vim.as_ref().and_then(|v| v.scroll);
        let st = cx.state::<EditorState>();
        st.view = size;
        // Vim scroll requests (`zz`, `Ctrl-e`, `Ctrl-d`) come before keeping the cursor in view.
        if let Some((serial, request)) = scroll_request
            && st.scroll_serial != serial {
                st.scroll_serial = serial;
                let cursor_top = cursor.line as f32 * line_h;
                match request {
                    Scroll::Center => st.scroll.y = cursor_top + PAD_Y + line_h * 0.5 - size.h * 0.5,
                    Scroll::Top => st.scroll.y = cursor_top,
                    Scroll::Bottom => st.scroll.y = cursor_top + line_h + PAD_Y * 2.0 - size.h,
                    Scroll::Lines(n) => st.scroll.y += n as f32 * line_h,
                }
                st.scroll.y = st.scroll.y.clamp(0.0, max_y);
            }
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

        let top = ((scroll.y - PAD_Y).max(0.0) / self.line_h).ceil() as usize;
        let whole = ((size.h - PAD_Y * 2.0) / self.line_h).floor().max(1.0) as usize;
        self.viewport = (top.min(self.lines.len() - 1), whole);

        // Lay out only the visible lines.
        self.first = ((scroll.y - PAD_Y) / self.line_h).floor().max(0.0) as usize;
        let count = (size.h / self.line_h).ceil() as usize + 2;
        let last = (self.first + count).min(self.lines.len());
        self.first = self.first.min(last);
        let mut in_comment = false;
        if self.language.has_block_comments() {
            for l in &self.lines[..self.first] {
                highlight(self.language, l, &mut in_comment);
            }
        }
        self.rows.clear();
        self.numbers.clear();
        let num_style = TextStyle { size: style.size * 0.92, ..style };
        for i in self.first..last {
            let line = &self.lines[i];
            let own = highlight(self.language, line, &mut in_comment);
            let start = self.tokens.partition_point(|t| t.line < i);
            let theirs: Vec<(usize, usize, Kind)> = self.tokens[start..].iter().take_while(|t| t.line == i).map(|t| (t.from, t.to, t.kind)).collect();
            let spans: Vec<(&str, Option<Color>)> = overlay(line, own, &theirs).into_iter().map(|(a, b, k)| (&line[a..b], colors.color(k))).collect();
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
        let block = self.vim.as_ref().and_then(|v| v.block);
        // Current line.
        if sel.is_none() && block.is_none() {
            let y = self.line_y(b, scroll, self.cursor.line);
            cx.scene.fill(Rect::new(b.x, y, b.w, self.line_h), 0.0, p.accent.with_alpha(if focused { 0.08 } else { 0.04 }), None);
        }

        cx.scene.push_clip(text_area);
        let tx = self.text_x(b, scroll);
        if let Some((s, e)) = sel {
            let linewise = self.selection.is_some_and(|x| x.2);
            let sel_color = p.accent.with_alpha(if focused { 0.28 } else { 0.16 });
            for line in s.line.max(self.first)..=e.line.min(self.first + self.rows.len().saturating_sub(1)) {
                let from = if line == s.line && !linewise { self.caret_x(line, s.col) } else { 0.0 };
                let to = if line == e.line && !linewise { self.caret_x(line, e.col) } else { self.caret_x(line, self.lines[line].len()) + self.char_w * 0.6 };
                if to > from {
                    let y = self.line_y(b, scroll, line);
                    cx.scene.fill(Rect::new(tx + from, y, to - from, self.line_h), 3.0, sel_color, None);
                }
            }
        }
        // The other selections, drawn the same way.
        for (s, e) in self.extras.iter().filter_map(|x| x.range) {
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
        if let Some(bl) = block {
            let sel_color = p.accent.with_alpha(if focused { 0.28 } else { 0.16 });
            for line in bl.first_line.max(self.first)..=bl.last_line.min(self.first + self.rows.len().saturating_sub(1)) {
                let text = &self.lines[line];
                let y = self.line_y(b, scroll, line);
                let (from, to) = match bl.bytes(text) {
                    Some((a, e)) => (self.caret_x(line, a), if e >= text.len() && !bl.to_end { self.caret_x(line, text.len()) } else { self.caret_x(line, e) }),
                    // Short lines: show the block's columns as empty space.
                    None => (bl.first_col as f32 * self.char_w, bl.first_col as f32 * self.char_w),
                };
                let to = if bl.to_end { to + self.char_w * 0.6 } else { to };
                if to > from {
                    cx.scene.fill(Rect::new(tx + from, y, to - from, self.line_h), 3.0, sel_color, None);
                }
            }
        }
        // Underlines, such as problems a language server reported.
        let last_row = self.first + self.rows.len().saturating_sub(1);
        for m in &self.marks {
            let (s, e) = (m.from.min(m.to), m.from.max(m.to));
            if e.line >= self.lines.len() {
                continue;
            }
            for line in s.line.max(self.first)..=e.line.min(last_row) {
                let from = if line == s.line { self.caret_x(line, s.col.min(self.lines[line].len())) } else { 0.0 };
                let to = if line == e.line { self.caret_x(line, e.col.min(self.lines[line].len())) } else { self.caret_x(line, self.lines[line].len()) };
                let y = self.line_y(b, scroll, line) + self.line_h - 3.0;
                cx.scene.fill(Rect::new(tx + from, y, (to - from).max(self.char_w), 2.0), 1.0, m.tone.resolve(p.text, &p), None);
            }
        }
        let default = p.text;
        for (i, layout) in self.rows.iter().enumerate() {
            let y = self.line_y(b, scroll, self.first + i);
            cx.scene.text(layout, Point::new(tx.round(), (y + (self.line_h - layout.size().h) * 0.5).round()), default);
        }
        // The other selections' carets, a little fainter than the main one.
        let shown = self.first..self.first + self.rows.len();
        for c in self.extras.iter().map(|x| x.cursor).filter(|c| shown.contains(&c.line)) {
            let x0 = self.caret_x(c.line, c.col);
            let y = self.line_y(b, scroll, c.line);
            if self.block_caret {
                let line = &self.lines[c.line];
                let x1 = line[c.col.min(line.len())..].chars().next().map_or(x0 + self.char_w, |ch| self.caret_x(c.line, c.col + ch.len_utf8()));
                let rect = Rect::new((tx + x0).round(), y + 1.0, (x1 - x0).max(self.char_w * 0.5).round(), self.line_h - 2.0);
                cx.scene.fill(rect, 2.0, p.accent_text.with_alpha(if focused { 0.3 } else { 0.15 }), None);
            } else if focused {
                cx.scene.fill(Rect::new((tx + x0).round() - 1.0, y + 2.0, 2.0, self.line_h - 4.0), 1.0, p.accent_text.with_alpha(0.7), None);
            }
        }
        if self.block_caret {
            // Modal keymaps outside Insert mode: a steady block over the character.
            let line = &self.lines[self.cursor.line];
            let x0 = self.caret_x(self.cursor.line, self.cursor.col);
            let x1 = if self.cursor.col < line.len() {
                let next = line[self.cursor.col..].chars().next().map_or(line.len(), |c| self.cursor.col + c.len_utf8());
                self.caret_x(self.cursor.line, next)
            } else {
                x0 + self.char_w
            };
            let y = self.line_y(b, scroll, self.cursor.line);
            let rect = Rect::new((tx + x0).round(), y + 1.0, (x1 - x0).max(self.char_w * 0.5).round(), self.line_h - 2.0);
            if focused {
                cx.scene.fill(rect, 2.0, p.accent_text.with_alpha(0.45), None);
            } else {
                cx.scene.fill(rect, 2.0, Color::TRANSPARENT, Some((1.0, p.accent_text.with_alpha(0.7))));
            }
        } else if focused {
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
        cx.scene.fill(sep, 0.0, p.line.with_alpha(0.7), None);
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
        // A popup at the caret, or where it was asked to be: under its line, or over it near the bottom.
        if !self.popup_rows.is_empty() {
            let row_h = self.line_h + 4.0;
            let wide = self.popup_rows.iter().map(|(l, d)| l.size().w + d.as_ref().map_or(0.0, |d| d.size().w + 18.0)).fold(0.0, f32::max);
            let size = Size::new((wide + 20.0).min(b.w - 16.0), self.popup_rows.len() as f32 * row_h + 8.0);
            let at = self.popup_at.unwrap_or(self.cursor);
            let line_top = self.line_y(b, scroll, at.line);
            let below = line_top + self.line_h + 2.0;
            let y = if below + size.h > b.bottom() - 4.0 && line_top - size.h - 2.0 > b.y { line_top - size.h - 2.0 } else { below };
            let x = (self.text_x(b, scroll) + self.caret_x(at.line, at.col)).min(b.right() - size.w - 8.0).max(b.x + 8.0);
            let rect = Rect::new(x.round(), y.round(), size.w, size.h);
            let picked = match &self.popup {
                Some(EditorPopup::List { selected, .. }) => selected.checked_sub(self.popup_first),
                _ => None,
            };
            cx.scene.push_layer();
            let radius = theme.small_radius() + 2.0;
            cx.scene.shadow(rect, radius, &neo_theme::Shadow { offset: (0.0, 6.0), blur: 20.0, spread: 0.0, color: Color::BLACK.with_alpha(0.22), inset: false });
            cx.scene.fill(rect, radius, p.surface, Some((1.0, p.line)));
            cx.scene.push_clip(rect);
            for (i, (label, detail)) in self.popup_rows.iter().enumerate() {
                let top = rect.y + 4.0 + i as f32 * row_h;
                if picked == Some(i) {
                    let mut paint = theme.paint(Surface::Pressed);
                    paint.shadows.clear();
                    cx.scene.paint(Rect::new(rect.x + 4.0, top, rect.w - 8.0, row_h), theme.small_radius(), &paint);
                }
                let text_y = (top + (row_h - label.size().h) * 0.5).round();
                cx.scene.text(label, Point::new(rect.x + 10.0, text_y), p.text);
                if let Some(d) = detail {
                    cx.scene.text(d, Point::new(rect.x + 10.0 + label.size().w + 18.0, text_y), p.muted);
                }
            }
            cx.scene.pop_clip();
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
        // While a popup shows, the keys that work it go to it.
        if let (Some(popup), Some(on_key), Event::Key(k)) = (&self.popup, &self.on_popup_key, event)
            && k.pressed
            && cx.is_focused()
        {
            let list = matches!(popup, EditorPopup::List { .. });
            let key = match &k.key {
                Key::Escape => Some(PopupKey::Dismiss),
                Key::Up if list => Some(PopupKey::Up),
                Key::Down if list => Some(PopupKey::Down),
                Key::Enter | Key::Tab if list => Some(PopupKey::Accept),
                _ => None,
            };
            if let Some(key) = key {
                cx.emit(on_key(key));
                return Status::Captured;
            }
        }
        match event {
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let st = cx.state::<EditorState>();
                st.scroll.x += delta.x;
                st.scroll.y += delta.y;
                cx.request_layout();
                // What was under the mouse has moved from under it.
                self.point(cx, None);
                Status::Captured
            }
            Event::PointerLeft => {
                self.point(cx, None);
                Status::Ignored
            }
            Event::PointerMoved { pos } => {
                let dragging = cx.state::<EditorState>().dragging;
                self.point(cx, if dragging { None } else { self.word_at(b, scroll, *pos) });
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
            Event::Key(k) if k.pressed && cx.is_focused() && self.vim.is_some() && !k.modifiers.logo && (!k.modifiers.ctrl || vim_ctrl(&k.key)) => {
                // A modal keymap: the document interprets every key. Cmd/Super
                // shortcuts, and Ctrl keys the keymaps do not use, fall through
                // to the clipboard and application shortcuts below.
                let view = self.vim.as_ref().expect("vim view present in vim mode");
                if view.viewport != self.viewport {
                    let (top, lines) = self.viewport;
                    send(cx, Action::Viewport { top, lines });
                }
                let paste = matches!(&k.key, Key::Character(c) if c == "p") && !k.modifiers.ctrl;
                let needs = match view.clipboard_need {
                    ClipboardNeed::Always => true,
                    ClipboardNeed::OnPaste => paste,
                    ClipboardNeed::None => false,
                };
                if needs
                    && let Some(text) = cx.read_clipboard() {
                        send(cx, Action::Clipboard(text));
                    }
                send(cx, Action::Key(k.clone()));
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
                            let Some(text) = self.selected_text() else { return Status::Captured };
                            cx.copy(text);
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
