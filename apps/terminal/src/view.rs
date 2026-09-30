//! The terminal grid widget: draws the screen and sends input to the pty.

use std::borrow::Cow;
use std::sync::Arc;

use alacritty_terminal::event::{EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoopSender, Msg as PtyMsg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::CursorShape;
use neo::widgets::style::Tone;
use neo::{Color, Cx, CursorIcon, DrawCx, Event, EventCx, FontFamily, Key, Length, Limits, Point, PointerButton, Rect, Size, Status, TextStyle, Widget};

use crate::colors::TermColors;
use crate::keys;

pub const PADDING: Size = Size { w: 12.0, h: 8.0 };

/// Font settings for the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f32,
    pub line_height: f32,
}

impl Font {
    fn style(&self, bold: bool) -> TextStyle {
        TextStyle { size: self.size, weight: if bold { 700 } else { 400 }, family: FontFamily::Mono, line_height: self.line_height, letter_spacing: 0.0 }
    }
}

#[derive(Default)]
struct ViewState {
    grid: (usize, usize),
    /// Fractional wheel scrolling carried between events.
    wheel: f32,
    selecting: bool,
    mounted: bool,
}

pub struct TermView<L: EventListener, M> {
    term: Arc<FairMutex<Term<L>>>,
    pty: EventLoopSender,
    font: Font,
    on_resize: Box<dyn Fn(WindowSize) -> M>,
    cell: Size,
}

impl<L: EventListener, M> TermView<L, M> {
    pub fn new(term: Arc<FairMutex<Term<L>>>, pty: EventLoopSender, font: Font, on_resize: impl Fn(WindowSize) -> M + 'static) -> Self {
        Self { term, pty, font, on_resize: Box::new(on_resize), cell: Size::new(8.0, 16.0) }
    }

    fn write(&self, bytes: Vec<u8>) {
        let _ = self.pty.send(PtyMsg::Input(Cow::Owned(bytes)));
    }

    /// The grid cell under a window position.
    fn cell_at(&self, bounds: Rect, p: Point, display_offset: usize, cols: usize, rows: usize) -> (GridPoint, Side) {
        let x = (p.x - bounds.x - PADDING.w).max(0.0);
        let y = (p.y - bounds.y - PADDING.h).max(0.0);
        let col = ((x / self.cell.w) as usize).min(cols.saturating_sub(1));
        let row = ((y / self.cell.h) as usize).min(rows.saturating_sub(1));
        let side = if x - col as f32 * self.cell.w < self.cell.w * 0.5 { Side::Left } else { Side::Right };
        (GridPoint::new(Line(row as i32 - display_offset as i32), Column(col)), side)
    }

    fn paste(&self, text: &str, bracketed: bool) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed { format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")) } else { text };
        self.write(bytes.into_bytes());
    }
}

impl<L: EventListener + 'static, M: 'static> Widget<M> for TermView<L, M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let probe = cx.text().layout("MMMMMMMMMM", &self.font.style(false), None);
        self.cell = Size::new(probe.size().w / 10.0, (self.font.size * self.font.line_height).round());
        let cols = (((size.w - PADDING.w * 2.0) / self.cell.w).floor() as usize).max(2);
        let rows = (((size.h - PADDING.h * 2.0) / self.cell.h).floor() as usize).max(1);
        let st = cx.state::<ViewState>();
        let first = !std::mem::replace(&mut st.mounted, true);
        if st.grid != (cols, rows) {
            st.grid = (cols, rows);
            let ws = WindowSize { num_cols: cols as u16, num_lines: rows as u16, cell_width: self.cell.w.round() as u16, cell_height: self.cell.h as u16 };
            cx.defer((self.on_resize)(ws));
        }
        if first {
            cx.request_focus();
        }
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let colors = TermColors::new(&p, theme.scheme);
        let focused = cx.is_focused();
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let origin = Point::new(b.x + PADDING.w, b.y + PADDING.h);
        let cw = self.cell.w;
        let ch = self.cell.h;
        // Round shared edges, not widths, so neighbouring cells meet without seams.
        let cell_rect = |line: i32, col: usize, width: usize| {
            let x0 = (origin.x + col as f32 * cw).round();
            let x1 = (origin.x + (col + width) as f32 * cw).round();
            Rect::new(x0, origin.y + line as f32 * ch, x1 - x0, ch)
        };

        cx.scene.push_clip(b);
        // Runs of text that share a colour and weight, each placed at its own
        // column so that wide or fallback glyphs cannot push later text out of line.
        struct Run {
            line: i32,
            col: usize,
            text: String,
            fg: Color,
            bold: bool,
            decorations: Flags,
        }
        let mut runs: Vec<Run> = Vec::new();
        let mut backgrounds: Vec<(i32, usize, usize, Color)> = Vec::new();
        let selection = content.selection;
        let cursor = content.cursor;
        let mut cursor_cell: Option<(char, Color, bool)> = None;
        for indexed in content.display_iter {
            let cell = indexed.cell;
            let line = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let bold = cell.flags.contains(Flags::BOLD);
            let mut fg_c = cell.fg;
            if bold {
                fg_c = colors.brighten(fg_c);
            }
            let mut fg = colors.resolve(fg_c, content.colors);
            let mut bg = colors.resolve(cell.bg, content.colors);
            let default_bg = matches!(cell.bg, alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background));
            let mut paint_bg = !default_bg;
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                paint_bg = true;
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.mix(colors.bg, 0.4);
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            let fill = if selection.is_some_and(|s| s.contains(indexed.point)) { Some(colors.selection) } else { paint_bg.then_some(bg) };
            if let Some(fill) = fill {
                match backgrounds.last_mut() {
                    Some((l, c, w, f)) if *l == line && *c + *w == col && *f == fill => *w += width,
                    _ => backgrounds.push((line, col, width, fill)),
                }
            }
            if indexed.point == cursor.point && cursor.shape != CursorShape::Hidden {
                cursor_cell = Some((cell.c, fg, bold));
            }
            if cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            let decorations = cell.flags & (Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
            let glyph = if cell.c == '\t' { ' ' } else { cell.c };
            let extend = runs.last_mut().filter(|r| r.line == line && r.fg == fg && r.bold == bold && r.decorations == decorations && r.col + r.text.chars().count() == col && width == 1 && r.text.is_ascii());
            match extend {
                Some(r) if glyph.is_ascii() => r.text.push(glyph),
                _ => runs.push(Run { line, col, text: glyph.to_string(), fg, bold, decorations }),
            }
            if let Some(extra) = cell.zerowidth()
                && let Some(r) = runs.last_mut()
            {
                r.text.extend(extra);
            }
        }

        for (line, col, w, color) in backgrounds {
            cx.scene.fill(cell_rect(line, col, w), 0.0, color, None);
        }
        for r in &runs {
            let trimmed = r.text.trim_end();
            if trimmed.is_empty() && r.decorations.is_empty() {
                continue;
            }
            let layout = cx.text().layout(trimmed, &self.font.style(r.bold), None);
            let at = cell_rect(r.line, r.col, 1);
            cx.scene.text(&layout, Point::new(at.x, at.y), r.fg);
            if !r.decorations.is_empty() {
                let w = r.text.chars().count() as f32 * cw;
                let y = if r.decorations.contains(Flags::STRIKEOUT) { at.y + ch * 0.55 } else { at.y + ch - 2.0 };
                cx.scene.fill(Rect::new(at.x, y.round(), w, 1.0), 0.0, r.fg, None);
            }
        }

        // The cursor, drawn over the text so a block cursor can invert its character.
        let cursor_line = cursor.point.line.0 + offset;
        if cursor.shape != CursorShape::Hidden && (0..term.screen_lines() as i32).contains(&cursor_line) {
            let at = cell_rect(cursor_line, cursor.point.column.0, 1);
            let shape = if focused { cursor.shape } else { CursorShape::HollowBlock };
            match shape {
                CursorShape::Block => {
                    cx.scene.fill(at, 2.0, colors.cursor, None);
                    if let Some((c, _, bold)) = cursor_cell.filter(|(c, ..)| !c.is_whitespace()) {
                        let layout = cx.text().layout(&c.to_string(), &self.font.style(bold), None);
                        cx.scene.text(&layout, Point::new(at.x, at.y), colors.on_cursor);
                    }
                }
                CursorShape::Beam => cx.scene.fill(Rect::new(at.x, at.y, 2.0, at.h), 0.0, colors.cursor, None),
                CursorShape::Underline => cx.scene.fill(Rect::new(at.x, at.bottom() - 2.0, at.w, 2.0), 0.0, colors.cursor, None),
                CursorShape::HollowBlock => cx.scene.fill(at, 2.0, Color::TRANSPARENT, Some((1.0, colors.cursor))),
                CursorShape::Hidden => {}
            }
        }

        // A hint while scrolled back through history.
        if offset > 0 {
            let label = cx.text().layout(&format!("↑ {offset} lines"), &TextStyle { size: 11.5, weight: 600, ..TextStyle::default() }, None);
            let s = label.size();
            let pill = Rect::new(b.right() - s.w - 34.0, b.y + 10.0, s.w + 20.0, s.h + 8.0);
            cx.scene.fill(pill, pill.h * 0.5, p.surface, Some((1.0, p.line)));
            cx.scene.text(&label, Point::new(pill.x + 10.0, pill.y + 4.0), Tone::Muted.resolve(p.text, &p));
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerMoved { pos } => {
                if b.contains(*pos) {
                    cx.set_cursor(CursorIcon::Text);
                }
                if cx.state::<ViewState>().selecting {
                    let mut term = self.term.lock();
                    let (point, side) = self.cell_at(b, *pos, term.grid().display_offset(), term.columns(), term.screen_lines());
                    if let Some(sel) = term.selection.as_mut() {
                        sel.update(point, side);
                    }
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.request_focus();
                let mut term = self.term.lock();
                let (point, side) = self.cell_at(b, *pos, term.grid().display_offset(), term.columns(), term.screen_lines());
                term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                cx.state::<ViewState>().selecting = true;
                cx.request_redraw();
                Status::Captured
            }
            Event::PointerReleased { button: PointerButton::Primary, .. } => {
                let st = cx.state::<ViewState>();
                if std::mem::take(&mut st.selecting) {
                    let mut term = self.term.lock();
                    if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
                        term.selection = None;
                    }
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let st = cx.state::<ViewState>();
                st.wheel += delta.y / self.cell.h;
                let lines = st.wheel.trunc() as i32;
                st.wheel -= lines as f32;
                if lines != 0 {
                    let mut term = self.term.lock();
                    let mode = *term.mode();
                    if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                        // Full-screen programs such as less get arrow keys instead.
                        let seq: &[u8] = if lines > 0 { if mode.contains(TermMode::APP_CURSOR) { b"\x1bOB" } else { b"\x1b[B" } } else if mode.contains(TermMode::APP_CURSOR) { b"\x1bOA" } else { b"\x1b[A" };
                        drop(term);
                        self.write(seq.repeat(lines.unsigned_abs() as usize));
                    } else {
                        term.scroll_display(Scroll::Delta(-lines));
                    }
                    cx.request_redraw();
                }
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let m = k.modifiers;
                // Copy and paste: Command on macOS, Ctrl+Shift elsewhere.
                let shortcut = if cfg!(target_os = "macos") { m.logo } else { m.ctrl && m.shift };
                if shortcut && let Key::Character(c) = &k.key {
                    match c.as_str() {
                        "c" => {
                            if let Some(text) = self.term.lock().selection_to_string() {
                                cx.copy(text);
                            }
                            return Status::Captured;
                        }
                        "v" => {
                            if let Some(text) = cx.read_clipboard() {
                                let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
                                self.paste(&text, bracketed);
                            }
                            return Status::Captured;
                        }
                        _ => {}
                    }
                }
                if m.shift && matches!(k.key, Key::PageUp | Key::PageDown) {
                    self.term.lock().scroll_display(if k.key == Key::PageUp { Scroll::PageUp } else { Scroll::PageDown });
                    cx.request_redraw();
                    return Status::Captured;
                }
                if m.logo {
                    // Leave Command and Super shortcuts to the app and desktop.
                    return Status::Ignored;
                }
                let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
                if let Some(bytes) = keys::encode(k, app_cursor) {
                    {
                        let mut term = self.term.lock();
                        term.selection = None;
                        term.scroll_display(Scroll::Bottom);
                    }
                    self.write(bytes);
                    cx.request_redraw();
                    return Status::Captured;
                }
                Status::Ignored
            }
            Event::Ime(text) if cx.is_focused() => {
                self.write(text.clone().into_bytes());
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
