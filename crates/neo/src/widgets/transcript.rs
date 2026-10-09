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

/// What has been said, oldest first, scrolling, and kept at its end as
/// more is said or the last of it grows.
pub fn transcript<M: 'static>(said: &[Said]) -> Element<M> {
    let mut all = column().spacing(14.0).width(Length::Fill).padding([14.0, 14.0]);
    for s in said {
        let entry: Element<M> = match s.who {
            // What one wrote oneself, set apart to the right as on a page of letters.
            Speaker::You => column().width(Length::Fill).align(Align::End).push(container(text(s.text.clone()).width(Length::Fill)).surface(Surface::Well).padding([12.0, 9.0]).max_width(560.0)).into(),
            Speaker::Them => text(s.text.clone()).width(Length::Fill).into(),
            Speaker::Note => text(s.text.clone()).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill).into(),
        };
        all = all.push(entry);
    }
    // Asked to show the last again whenever there is more of anything.
    let more = said.len() as u64 + said.iter().map(|s| s.text.len() as u64).sum::<u64>();
    scrollable(all).reveal(more, said.len().saturating_sub(1)).into()
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
