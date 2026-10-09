//! A conversation: what has been said, scrolling, and a place to write
//! the next thing in.
//!
//! For talking to something that answers at length and as it thinks: an
//! assistant, an agent at work. What has been said is a list the app
//! keeps and adds to; the last of it can grow as it arrives.

use armature::document::{Action, Document};
use armature_render::{Point, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::style::Tone;
use super::text_editor::text_editor;
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

/// The size of the writing in a prompt, the room round it, and how tall
/// one line of it is to the size.
const WRITING: f32 = 14.0;
const PAD: (f32, f32) = (14.0, 10.0);
const LINE: f32 = 1.6;

/// A place to write the next thing to say: see [`prompt`].
pub struct Prompt<M> {
    editor: [Element<M>; 1],
    on_submit: Option<M>,
    placeholder: String,
    hint: Option<TextLayout>,
    lines: usize,
    most: usize,
    empty: bool,
}

/// A few lines to write in, growing with what is written up to a height
/// and scrolling after. Enter sends it; Shift with Enter starts a new
/// line. The writing is a `Document` the app keeps: `on_edit` is told
/// each change to make to it with `Document::apply`, and after sending
/// the app starts a new one.
pub fn prompt<M: Clone + 'static>(doc: &Document, on_edit: impl Fn(Action) -> M + 'static) -> Prompt<M> {
    let empty = doc.lines().iter().all(String::is_empty);
    Prompt { editor: [text_editor(doc).gutter(false).font_size(WRITING).on_action(on_edit).width(Length::Fill).into()], on_submit: None, placeholder: String::new(), hint: None, lines: doc.line_count().max(1), most: 8, empty }
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
}

impl<M: Clone + 'static> Widget<M> for Prompt<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.editor
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let line = (WRITING * cx.theme().text_scale * LINE).ceil();
        let height = self.lines.clamp(1, self.most) as f32 * line + PAD.1 * 2.0;
        let size = limits.constrain(Length::Fill, Length::Shrink).resolve(Size::new(320.0, height));
        self.editor[0].layout(cx, Limits::tight(size));
        self.editor[0].set_position(Point::ZERO);
        let style = armature_render::TextStyle { size: WRITING * cx.theme().text_scale, weight: 400, family: armature_render::FontFamily::Mono, line_height: LINE, letter_spacing: 0.0 };
        self.hint = (self.empty && !self.placeholder.is_empty()).then(|| cx.text().layout(&self.placeholder, &style, None));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        cx.scene.paint(b, theme.small_radius(), &theme.paint(Surface::Inset));
        cx.scene.push_clip(b);
        self.editor[0].draw(cx);
        if let Some(hint) = &self.hint {
            cx.scene.text(hint, Point::new(b.x + PAD.0, b.y + PAD.1), theme.palette().faint);
        }
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        // While it is being written in, Enter sends what is written; with Shift it is a new line.
        if let Event::Key(k) = event
            && k.pressed
            && k.key == Key::Enter
            && !k.modifiers.shift
            && !k.modifiers.alt
            && self.editor[0].has_focus(cx)
        {
            if let (Some(m), false) = (&self.on_submit, self.empty) {
                cx.emit(m.clone());
            }
            return Status::Captured;
        }
        Element::event_children(&mut self.editor, cx, event)
    }
}

impl<M: Clone + 'static> From<Prompt<M>> for Element<M> {
    fn from(w: Prompt<M>) -> Self {
        Element::new(w)
    }
}
