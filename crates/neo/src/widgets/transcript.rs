//! A conversation: what has been said, scrolling, and a place to write
//! the next thing in.
//!
//! For talking to something that answers at length and as it thinks: an
//! assistant, an agent at work. What has been said is a list the app
//! keeps and adds to; the last of it can grow as it arrives.

use armature::document::{Action, Document, Pos};
use armature_render::{Point, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::style::Tone;
use super::{column, container, scrollable, text};
use crate::ThemeCx;
use crate::core::{Align, Cx, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, Status};

/// Who said something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    /// The person at the keyboard.
    You,
    /// Whoever answers.
    Them,
    /// Neither: a remark on what is going on, such as "Stopped".
    Note,
}

/// One thing said in a conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Said {
    pub who: Speaker,
    pub text: String,
}

impl Said {
    pub fn new(who: Speaker, text: impl Into<String>) -> Self {
        Self { who, text: text.into() }
    }
}

/// How a piece of work asked of a tool stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// A piece of work whoever answers asked a tool to do: a file read, a
/// command run, a picture taken. Shut it is one line; open it shows what
/// the tool was given and what came back.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow<Id> {
    /// The app's own name for it, given back when it is opened or shut.
    pub id: Id,
    pub name: String,
    /// What it is doing, in a few words, beside its name.
    pub summary: String,
    pub state: ToolState,
    pub open: bool,
    /// What it was given and what came back, as text; either may be
    /// empty, and may still be arriving.
    pub input: String,
    pub result: String,
    /// A picture that came back, shown under the text.
    pub image: Option<armature_render::Image>,
}

impl<Id> ToolRow<Id> {
    /// A tool just asked for: running, shut, with nothing to show yet.
    pub fn new(id: Id, name: impl Into<String>, summary: impl Into<String>) -> Self {
        Self { id, name: name.into(), summary: summary.into(), state: ToolState::Running, open: false, input: String::new(), result: String::new(), image: None }
    }
}

/// Something asked of the person at the keyboard before going on: leave
/// to run a tool, say. Answered, the app puts a [`Speaker::Note`] in its place.
#[derive(Clone, Debug)]
pub struct Asking<M> {
    /// What is asked, in a line.
    pub what: String,
    /// The particulars, in fixed-width type: a command, a path.
    pub detail: String,
    /// The answers there are: what each button says and sends. The first
    /// is the one offered as the usual answer.
    pub choices: Vec<(String, M)>,
}

/// One thing in a conversation.
#[derive(Clone, Debug)]
pub enum Entry<Id, M> {
    Said(Said),
    Tool(ToolRow<Id>),
    Ask(Asking<M>),
}

/// One thing said. What whoever answers says is read as Markdown, since
/// that is what such things write; `on_link` hears of a link in it clicked.
fn said<M: 'static>(s: &Said, on_link: Option<std::rc::Rc<dyn Fn(String) -> M>>) -> Element<M> {
    match s.who {
        // What one wrote oneself, set apart to the right as on a page of letters.
        Speaker::You => column().width(Length::Fill).align(Align::End).push(container(text(s.text.clone()).width(Length::Fill)).surface(Surface::Well).padding([12.0, 9.0]).max_width(560.0)).into(),
        Speaker::Them => match on_link {
            Some(f) => super::markdown(&s.text).on_link(move |to| f(to)).into(),
            None => super::markdown(&s.text).into(),
        },
        Speaker::Note => text(s.text.clone()).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill).into(),
    }
}

/// What has been said, oldest first, scrolling. It stays at its end as
/// more is said or the last of it grows; scrolled back to read something
/// earlier, it stays there until it is scrolled to the end again.
pub fn transcript<M: 'static>(all: &[Said]) -> Element<M> {
    let mut entries = column().spacing(14.0).width(Length::Fill).padding([14.0, 14.0]);
    for s in all {
        entries = entries.push(said(s, None));
    }
    scrollable(entries).follow_end(true).into()
}

/// A conversation with something that works as it answers: what is said,
/// the tools it uses, each a row that opens to show what went in and what
/// came out, and what it asks leave for. `on_toggle` is told that a
/// tool's row is to be opened (true) or shut; which are open is the
/// app's to keep, in each [`ToolRow`].
pub fn conversation<Id: Clone + 'static, M: Clone + 'static>(entries: &[Entry<Id, M>], on_toggle: impl Fn(Id, bool) -> M + Clone + 'static) -> Element<M> {
    converse(entries, on_toggle, None)
}

/// As [`conversation`], and told the address of a link clicked in what
/// whoever answers has said. What following it means is the app's to say.
pub fn conversation_linked<Id: Clone + 'static, M: Clone + 'static>(entries: &[Entry<Id, M>], on_toggle: impl Fn(Id, bool) -> M + Clone + 'static, on_link: impl Fn(String) -> M + 'static) -> Element<M> {
    converse(entries, on_toggle, Some(std::rc::Rc::new(on_link)))
}

fn converse<Id: Clone + 'static, M: Clone + 'static>(entries: &[Entry<Id, M>], on_toggle: impl Fn(Id, bool) -> M + Clone + 'static, on_link: Option<std::rc::Rc<dyn Fn(String) -> M>>) -> Element<M> {
    use super::{button, icon, mouse_area, picture, row, Button, ButtonKind, Fit};
    use neo_theme::icons;
    let mut all = column().spacing(14.0).width(Length::Fill).padding([14.0, 14.0]);
    for entry in entries {
        let shown: Element<M> = match entry {
            Entry::Said(s) => said(s, on_link.clone()),
            Entry::Tool(t) => {
                let (mark, tone) = match t.state {
                    ToolState::Running => (icons::LOADER, Tone::Muted),
                    ToolState::Done => (icons::CIRCLE_CHECK, Tone::Good),
                    ToolState::Failed => (icons::CIRCLE_X, Tone::Bad),
                };
                let head = row()
                    .spacing(8.0)
                    .align(Align::Center)
                    .width(Length::Fill)
                    .push(icon(if t.open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }).size(14.0).tone(Tone::Muted))
                    .push(icon(mark).size(14.0).tone(tone))
                    .push(text(t.name.clone()).role(TextRole::Strong))
                    .push(text(t.summary.clone()).tone(Tone::Muted).no_wrap().width(Length::Fill));
                let (say, id, open) = (on_toggle.clone(), t.id.clone(), t.open);
                let mut body = column().spacing(8.0).width(Length::Fill).push(mouse_area(head).on_press(move || say(id.clone(), !open)));
                if t.open {
                    for (label, what) in [("Given", &t.input), ("Came back", &t.result)] {
                        if !what.is_empty() {
                            body = body.push(text(label).role(TextRole::Label).tone(Tone::Muted)).push(text(what.clone()).mono().size(12.5).width(Length::Fill));
                        }
                    }
                    if let Some(image) = &t.image {
                        body = body.push(picture(image).fit(Fit::Contain).width(Length::Fill).height(240.0));
                    }
                    if t.input.is_empty() && t.result.is_empty() && t.image.is_none() {
                        body = body.push(text(if t.state == ToolState::Running { "Nothing yet." } else { "Nothing to show." }).role(TextRole::Caption).tone(Tone::Faint));
                    }
                }
                container(body).surface(Surface::Well).padding([12.0, 9.0]).width(Length::Fill).into()
            }
            Entry::Ask(a) => {
                let mut answers = row().spacing(8.0).align(Align::Center);
                for (i, (label, m)) in a.choices.iter().enumerate() {
                    answers = answers.push(if i == 0 { Button::new(text(label.clone()).role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(m.clone()) } else { button(label.clone()).on_press(m.clone()) });
                }
                let mut body = column().spacing(10.0).width(Length::Fill).push(text(a.what.clone()).role(TextRole::Strong).width(Length::Fill));
                if !a.detail.is_empty() {
                    body = body.push(text(a.detail.clone()).mono().size(12.5).width(Length::Fill));
                }
                container(body.push(answers)).surface(Surface::Card).padding([14.0, 12.0]).width(Length::Fill).into()
            }
        };
        all = all.push(shown);
    }
    scrollable(all).follow_end(true).into()
}

/// The room round the writing in a prompt.
const PAD: (f32, f32) = (12.0, 9.0);

#[derive(Default)]
struct PromptState {
    /// How far down what is written has been scrolled, to keep the caret in sight.
    scroll: f32,
    dragging: bool,
    /// When it was last clicked, for telling a second click from a first.
    clicked: Option<std::time::Instant>,
}

/// A place to write the next thing to say: see [`prompt`].
pub struct Prompt<M> {
    lines: Vec<String>,
    cursor: Pos,
    selection: Option<(Pos, Pos)>,
    on_edit: Box<dyn Fn(Action) -> M>,
    on_submit: Option<M>,
    placeholder: String,
    most: usize,
    /// Each line of what is written, wrapped to the width, and where it starts.
    laid: Vec<(TextLayout, f32)>,
    hint: Option<TextLayout>,
    line_h: f32,
}

/// A few lines to write in, as prose: wrapped to the width, growing with
/// what is written up to a height and scrolling after. Enter sends it;
/// Shift with Enter starts a new line. The writing is a `Document` the
/// app keeps: `on_edit` is told each change to make to it with
/// `Document::apply`, and after sending the app starts a new one.
pub fn prompt<M: Clone + 'static>(doc: &Document, on_edit: impl Fn(Action) -> M + 'static) -> Prompt<M> {
    Prompt { lines: doc.lines().to_vec(), cursor: doc.cursor(), selection: doc.selection(), on_edit: Box::new(on_edit), on_submit: None, placeholder: String::new(), most: 8, laid: vec![], hint: None, line_h: 20.0 }
}

/// The byte a character of a line begins at.
fn byte_of(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

impl<M> Prompt<M> {
    /// What Enter sends, when there is something written.
    pub fn on_submit(mut self, m: M) -> Self {
        self.on_submit = Some(m);
        self
    }

    /// What is shown in it while nothing is written.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// How many lines it grows to before it scrolls: eight, to begin with.
    pub fn max_lines(mut self, most: usize) -> Self {
        self.most = most.max(1);
        self
    }

    fn empty(&self) -> bool {
        self.lines.iter().all(String::is_empty)
    }

    /// Where a place in the writing is, from the top left of all of it.
    fn point_of(&self, pos: Pos) -> Point {
        let Some((layout, top)) = self.laid.get(pos.line) else { return Point::ZERO };
        let at = layout.caret(byte_of(&self.lines[pos.line], pos.col));
        Point::new(at.x, top + at.y)
    }

    /// The place in the writing nearest a point, measured the same way.
    fn pos_at(&self, p: Point) -> Pos {
        let line = self.laid.iter().rposition(|(_, top)| p.y >= *top).unwrap_or(0);
        let Some((layout, top)) = self.laid.get(line) else { return Pos::new(0, 0) };
        let byte = layout.hit(Point::new(p.x.max(0.0), (p.y - top).clamp(0.0, (layout.size().h - 1.0).max(0.0))));
        let text = &self.lines[line];
        Pos::new(line, text[..byte.min(text.len())].chars().count())
    }

    fn written_height(&self) -> f32 {
        self.laid.last().map_or(self.line_h, |(l, top)| top + l.size().h)
    }

    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let mut out = String::new();
        for line in a.line..=b.line.min(self.lines.len().saturating_sub(1)) {
            let text = &self.lines[line];
            let from = if line == a.line { byte_of(text, a.col) } else { 0 };
            let to = if line == b.line { byte_of(text, b.col) } else { text.len() };
            out.push_str(&text[from.min(to)..to]);
            if line != b.line {
                out.push('\n');
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

impl<M: Clone + 'static> Widget<M> for Prompt<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = cx.theme().text(TextRole::Body).style();
        let width = limits.constrain(Length::Fill, Length::Shrink).max.w;
        let inner = (width - PAD.0 * 2.0).max(40.0);
        self.line_h = (style.size * style.line_height).round();
        let mut top = 0.0;
        self.laid = self
            .lines
            .iter()
            .map(|line| {
                let layout = cx.text().layout(line, &style, Some(inner));
                let at = top;
                top += layout.size().h.max(self.line_h);
                (layout, at)
            })
            .collect();
        self.hint = (self.empty() && !self.placeholder.is_empty()).then(|| cx.text().layout(&self.placeholder, &style, Some(inner)));
        let shown = self.written_height().clamp(self.line_h, self.most as f32 * self.line_h);
        // The caret stays in sight as what is written outgrows its place.
        let caret = self.point_of(self.cursor).y;
        let most = (self.written_height() - shown).max(0.0);
        let st = cx.state::<PromptState>();
        st.scroll = st.scroll.clamp((caret + self.line_h - shown).max(0.0), caret.max(0.0)).min(most);
        Size::new(width, shown + PAD.1 * 2.0)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let scroll = cx.state::<PromptState>().scroll;
        cx.scene.paint(b, theme.small_radius(), &theme.paint(Surface::Inset));
        if cx.is_focused() {
            cx.scene.fill(b, theme.small_radius(), armature_render::Color::TRANSPARENT, Some((1.5, p.accent_text.with_alpha(0.7))));
        }
        let origin = Point::new(b.x + PAD.0, b.y + PAD.1 - scroll);
        cx.scene.push_clip(armature_render::Rect::new(b.x, b.y + 2.0, b.w, b.h - 4.0));
        // What is selected, a band to each row of it.
        if let Some((from, to)) = self.selection {
            let (a, z) = (self.point_of(from), self.point_of(to));
            let right = b.w - PAD.0 * 2.0;
            let band = |cx: &mut DrawCx, x0: f32, x1: f32, y: f32| cx.scene.fill(armature_render::Rect::new(origin.x + x0, origin.y + y, (x1 - x0).max(2.0), self.line_h), 2.0, p.accent.with_alpha(0.3), None);
            if a.y == z.y {
                band(cx, a.x, z.x, a.y);
            } else {
                band(cx, a.x, right, a.y);
                let mut y = a.y + self.line_h;
                while y < z.y {
                    band(cx, 0.0, right, y);
                    y += self.line_h;
                }
                band(cx, 0.0, z.x, z.y);
            }
        }
        for (layout, top) in &self.laid {
            cx.scene.text(layout, Point::new(origin.x, origin.y + top), p.text);
        }
        if let Some(hint) = &self.hint {
            cx.scene.text(hint, origin, p.faint);
        }
        if cx.is_focused() {
            let at = self.point_of(self.cursor);
            cx.scene.fill(armature_render::Rect::new((origin.x + at.x).round() - 0.75, origin.y + at.y + 1.0, 1.5, self.line_h - 2.0), 0.75, p.accent_text, None);
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        use crate::event::PointerButton;
        use armature::document::Motion;
        let b = cx.bounds();
        let scroll = cx.state::<PromptState>().scroll;
        let within = |p: Point| Point::new(p.x - b.x - PAD.0, p.y - b.y - PAD.1 + scroll);
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.request_focus();
                let at = self.pos_at(within(*pos));
                let now = cx.now();
                let st = cx.state::<PromptState>();
                let twice = st.clicked.replace(now).is_some_and(|t| now.saturating_duration_since(t) < std::time::Duration::from_millis(400));
                st.dragging = true;
                cx.emit((self.on_edit)(if twice { Action::SelectWord(at) } else { Action::Click { pos: at, select: cx.modifiers().shift } }));
                Status::Captured
            }
            Event::PointerMoved { pos } => {
                if cx.state::<PromptState>().dragging {
                    cx.emit((self.on_edit)(Action::Drag(self.pos_at(within(*pos)))));
                    return Status::Captured;
                }
                if b.contains(*pos) {
                    cx.set_cursor(crate::core::CursorIcon::Text);
                }
                Status::Ignored
            }
            Event::PointerReleased { .. } => {
                cx.state::<PromptState>().dragging = false;
                Status::Ignored
            }
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                let most = (self.written_height() - (b.h - PAD.1 * 2.0)).max(0.0);
                if most <= 0.0 {
                    return Status::Ignored;
                }
                let st = cx.state::<PromptState>();
                st.scroll = (st.scroll - delta.y).clamp(0.0, most);
                cx.request_redraw();
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let m = k.modifiers;
                let cmd = m.command();
                let word = if cfg!(target_os = "macos") { m.alt } else { m.ctrl };
                let select = m.shift;
                let motion = |mo: Motion| Action::Move { motion: mo, select };
                // Up and down go by the rows as they are wrapped, not by the lines as they were written.
                let row = |this: &Self, by: f32| {
                    let at = this.point_of(this.cursor);
                    let y = at.y + by * this.line_h;
                    if y < 0.0 {
                        motion(Motion::DocStart)
                    } else if y >= this.written_height() {
                        motion(Motion::DocEnd)
                    } else {
                        Action::Click { pos: this.pos_at(Point::new(at.x + 1.0, y + this.line_h * 0.5)), select }
                    }
                };
                let action = match &k.key {
                    // Enter sends what is written; with Shift or Alt it is a new line.
                    Key::Enter if !m.shift && !m.alt => {
                        if let (Some(send), false) = (&self.on_submit, self.empty()) {
                            cx.emit(send.clone());
                        }
                        return Status::Captured;
                    }
                    Key::Enter => Action::Enter,
                    Key::Left if cmd => motion(Motion::LineStart),
                    Key::Right if cmd => motion(Motion::LineEnd),
                    Key::Up if cmd => motion(Motion::DocStart),
                    Key::Down if cmd => motion(Motion::DocEnd),
                    Key::Left if word => motion(Motion::WordLeft),
                    Key::Right if word => motion(Motion::WordRight),
                    Key::Left => motion(Motion::Left),
                    Key::Right => motion(Motion::Right),
                    Key::Up => row(self, -1.0),
                    Key::Down => row(self, 1.0),
                    Key::Home => motion(if cmd { Motion::DocStart } else { Motion::LineStart }),
                    Key::End => motion(if cmd { Motion::DocEnd } else { Motion::LineEnd }),
                    Key::Escape if self.selection.is_some() => Action::Collapse,
                    Key::Escape => {
                        cx.release_focus();
                        cx.request_redraw();
                        return Status::Captured;
                    }
                    Key::Backspace | Key::Delete => {
                        if self.selection.is_none() && (word || cmd) {
                            let mo = match (&k.key, cmd) {
                                (Key::Backspace, true) => Motion::LineStart,
                                (Key::Backspace, false) => Motion::WordLeft,
                                (_, true) => Motion::LineEnd,
                                _ => Motion::WordRight,
                            };
                            cx.emit((self.on_edit)(Action::Move { motion: mo, select: true }));
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
                        "v" => match cx.read_clipboard() {
                            Some(t) => Action::Insert(t),
                            None => return Status::Captured,
                        },
                        _ => return Status::Ignored,
                    },
                    // Tab is for moving on to the next thing, as in any field.
                    Key::Tab => return Status::Ignored,
                    _ => match k.text.as_deref().filter(|t| !t.chars().any(char::is_control) && !m.ctrl && !m.logo) {
                        Some(t) => Action::Insert(t.to_owned()),
                        None => return Status::Ignored,
                    },
                };
                cx.emit((self.on_edit)(action));
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

impl<M: Clone + 'static> From<Prompt<M>> for Element<M> {
    fn from(w: Prompt<M>) -> Self {
        Element::new(w)
    }
}
