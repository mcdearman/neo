//! A tree of things inside things: the entities of a scene, the folders of
//! a project. Rows open and close, one is selected, a name is edited where
//! it stands, and a row is dragged into another or between two.
//!
//! The tree is the app's: which rows are open, which is selected and what
//! is being typed for a name are all part of what the app gives, and the
//! widget only says what the user did.

use std::rc::Rc;

use armature_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Surface, TextRole};

use super::text_input::text_input;
use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};
use crate::{FocusRing, ThemeCx};

/// One thing in a tree, with whatever is inside it. `Id` is the app's own
/// name for it, handed back with everything that is done to it.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode<Id> {
    pub id: Id,
    pub label: String,
    pub icon: Option<neo_theme::Icon>,
    /// Whether what is inside it shows.
    pub open: bool,
    pub children: Vec<TreeNode<Id>>,
}

impl<Id> TreeNode<Id> {
    /// A thing with nothing inside it.
    pub fn new(id: Id, label: impl Into<String>) -> Self {
        Self { id, label: label.into(), icon: None, open: false, children: vec![] }
    }

    pub fn icon(mut self, icon: neo_theme::Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// With these inside it, shown or not.
    pub fn with(mut self, open: bool, children: Vec<TreeNode<Id>>) -> Self {
        (self.open, self.children) = (open, children);
        self
    }
}

/// Where a dragged row is let go, against the row it is let go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Inside it, as the last thing there.
    Into,
    /// Beside it, just above or just below.
    Before,
    After,
}

/// What is done to a name being edited in a tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeEdit<Id> {
    /// Asked for: a double click on the row, or Enter or F2 with it selected.
    Begin(Id),
    Typed(String),
    /// Enter: the name is to be what was typed.
    Done,
    /// Escape: it is to stay what it was.
    Dropped,
}

/// A row as it is shown: a node, and how deep it is.
struct Row<Id> {
    id: Id,
    depth: usize,
    open: bool,
    parent: bool,
    icon: Option<neo_theme::Icon>,
    /// How many rows there are inside it, shown: those just after it.
    inside: usize,
}

const ROW: f32 = 26.0;
const INDENT: f32 = 16.0;
const SLACK: f32 = 5.0;

#[derive(Default)]
struct TreeState {
    hovered: Option<usize>,
    /// A row pressed, and where; `moving` once dragged far enough.
    pressed: Option<(usize, Point)>,
    moving: bool,
    pointer: Point,
    /// When a row was last clicked, and which: twice quickly is a double click.
    clicked: Option<(usize, std::time::Instant)>,
}

/// What is said when one row is dragged to another.
type Moved<Id, M> = Rc<dyn Fn(Id, Id, Place) -> M>;

/// Shows a tree: see [`tree`].
pub struct Tree<Id, M> {
    rows: Vec<Row<Id>>,
    labels: Vec<String>,
    selected: Option<Id>,
    on_select: Option<Rc<dyn Fn(Id) -> M>>,
    on_toggle: Option<Rc<dyn Fn(Id, bool) -> M>>,
    on_move: Option<Moved<Id, M>>,
    on_edit: Option<Rc<dyn Fn(TreeEdit<Id>) -> M>>,
    /// The row whose name is being edited, and the field it is edited in.
    editing: Option<(usize, String)>,
    field: Vec<Element<M>>,
    layouts: Vec<TextLayout>,
    chevrons: Option<(TextLayout, TextLayout)>,
    icons: Vec<Option<TextLayout>>,
}

/// A tree of `nodes`, with the one `selected` marked. It is as tall as its
/// rows, to be put in something that scrolls.
pub fn tree<Id: Clone + PartialEq + 'static, M>(nodes: &[TreeNode<Id>], selected: Option<&Id>) -> Tree<Id, M> {
    fn flatten<Id: Clone>(nodes: &[TreeNode<Id>], depth: usize, rows: &mut Vec<Row<Id>>, labels: &mut Vec<String>) {
        for n in nodes {
            let at = rows.len();
            rows.push(Row { id: n.id.clone(), depth, open: n.open, parent: !n.children.is_empty(), icon: n.icon, inside: 0 });
            labels.push(n.label.clone());
            if n.open {
                flatten(&n.children, depth + 1, rows, labels);
            }
            rows[at].inside = rows.len() - at - 1;
        }
    }
    let (mut rows, mut labels) = (vec![], vec![]);
    flatten(nodes, 0, &mut rows, &mut labels);
    Tree { rows, labels, selected: selected.cloned(), on_select: None, on_toggle: None, on_move: None, on_edit: None, editing: None, field: vec![], layouts: vec![], chevrons: None, icons: vec![] }
}

impl<Id: Clone + PartialEq + 'static, M: Clone + 'static> Tree<Id, M> {
    /// Told which row was chosen, by a click or the arrow keys.
    pub fn on_select(mut self, f: impl Fn(Id) -> M + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Told that a row is to be opened (true) or closed.
    pub fn on_toggle(mut self, f: impl Fn(Id, bool) -> M + 'static) -> Self {
        self.on_toggle = Some(Rc::new(f));
        self
    }

    /// Told that the first row was dragged to the second: into it, or just
    /// before or after it. Never a row onto itself or into what is inside it.
    pub fn on_move(mut self, f: impl Fn(Id, Id, Place) -> M + 'static) -> Self {
        self.on_move = Some(Rc::new(f));
        self
    }

    /// Lets names be edited where they stand, and says what is done: see
    /// [`TreeEdit`]. The app keeps which is being edited and what has been
    /// typed, and gives them back with [`editing`](Self::editing).
    pub fn on_edit(mut self, f: impl Fn(TreeEdit<Id>) -> M + 'static) -> Self {
        self.on_edit = Some(Rc::new(f));
        self
    }

    /// The row whose name is being edited, and what has been typed for it.
    /// Call after [`on_edit`](Self::on_edit).
    pub fn editing(mut self, id: Option<&Id>, typed: &str) -> Self {
        self.editing = id.and_then(|id| self.rows.iter().position(|r| r.id == *id)).map(|i| (i, typed.to_owned()));
        if let (Some((_, typed)), Some(say)) = (&self.editing, self.on_edit.clone()) {
            let typing = say.clone();
            self.field = vec![text_input("", typed.clone()).on_input(move |t| typing(TreeEdit::Typed(t))).on_submit(say(TreeEdit::Done)).on_cancel(say(TreeEdit::Dropped)).autofocus(true).width(Length::Fill).into()];
        }
        self
    }

    fn row_at(&self, bounds: Rect, p: Point) -> Option<usize> {
        (bounds.contains(p)).then(|| ((p.y - bounds.y) / ROW) as usize).filter(|i| *i < self.rows.len())
    }

    /// Where a row dragged to `p` would go: against which row, and how.
    /// Nowhere, if that is itself or something inside it.
    fn target(&self, bounds: Rect, dragged: usize, p: Point) -> Option<(usize, Place)> {
        let onto = self.row_at(bounds, p)?;
        if onto >= dragged && onto <= dragged + self.rows[dragged].inside {
            return None;
        }
        let within = (p.y - bounds.y) / ROW - onto as f32;
        Some((onto, if within < 0.25 { Place::Before } else if within > 0.75 { Place::After } else { Place::Into }))
    }

    fn selected_row(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|s| self.rows.iter().position(|r| r.id == *s))
    }
}

impl<Id: Clone + PartialEq + 'static, M: Clone + 'static> Widget<M> for Tree<Id, M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn focusable(&self) -> bool {
        true
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.field
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = cx.theme().text(TextRole::Body).style();
        self.layouts = self.labels.iter().map(|l| cx.text().layout(l, &style, None)).collect();
        let glyphs = TextStyle { size: 14.0 * cx.theme().text_scale, weight: 400, family: FontFamily::Icons, line_height: 1.0, letter_spacing: 0.0 };
        self.chevrons = Some((cx.text().layout(&neo_theme::icons::CHEVRON_RIGHT.0.to_string(), &glyphs, None), cx.text().layout(&neo_theme::icons::CHEVRON_DOWN.0.to_string(), &glyphs, None)));
        self.icons = self.rows.iter().map(|r| r.icon.map(|i| cx.text().layout(&i.0.to_string(), &glyphs, None))).collect();
        let size = limits.constrain(Length::Fill, Length::Shrink).resolve(Size::new(240.0, self.rows.len() as f32 * ROW));
        // The field a name is edited in has the row from where the name starts.
        if let (Some((i, _)), Some(field)) = (&self.editing, self.field.first_mut()) {
            let x = self.rows[*i].depth as f32 * INDENT + INDENT + if self.rows[*i].icon.is_some() { 22.0 } else { 0.0 };
            field.layout(cx, Limits::tight(Size::new((size.w - x - 4.0).max(40.0), ROW)));
            field.set_position(Point::new(x, *i as f32 * ROW));
        }
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let (hovered, pressed, moving, pointer) = {
            let st = cx.state::<TreeState>();
            (st.hovered, st.pressed, st.moving, st.pointer)
        };
        let selected = self.selected_row();
        for (i, row) in self.rows.iter().enumerate() {
            let r = Rect::new(b.x, b.y + i as f32 * ROW, b.w, ROW);
            if selected == Some(i) {
                cx.scene.paint(r.inset(1.0), theme.small_radius(), &theme.paint(Surface::Pressed));
            } else if hovered == Some(i) && !moving {
                cx.scene.paint(r.inset(1.0), theme.small_radius(), &theme.paint(Surface::Hovered));
            }
            let mut x = r.x + row.depth as f32 * INDENT;
            if let (true, Some((shut, open))) = (row.parent, &self.chevrons) {
                let glyph = if row.open { open } else { shut };
                let s = glyph.size();
                cx.scene.text(glyph, Point::new(x + (INDENT - s.w) * 0.5, r.y + (ROW - s.h) * 0.5), p.muted);
            }
            x += INDENT;
            if let Some(Some(glyph)) = self.icons.get(i) {
                let s = glyph.size();
                cx.scene.text(glyph, Point::new(x + 2.0, r.y + (ROW - s.h) * 0.5), if selected == Some(i) { p.accent_text } else { p.muted });
                x += 22.0;
            }
            // The name, unless it is being edited, where the field stands instead.
            if self.editing.as_ref().is_none_or(|(e, _)| *e != i) {
                let s = self.layouts[i].size();
                cx.scene.push_clip(r);
                cx.scene.text(&self.layouts[i], Point::new(x + 2.0, r.y + ((ROW - s.h) * 0.5).round()), if selected == Some(i) { p.accent_text } else { p.text });
                cx.scene.pop_clip();
            }
        }
        if let Some(field) = self.field.first() {
            field.draw(cx);
        }
        cx.focus_ring(b, theme.small_radius());
        // Where a row being dragged would go: a box round the one it goes into, or a line beside it.
        if let (true, Some((dragged, _))) = (moving, pressed)
            && let Some((onto, place)) = self.target(b, dragged, pointer)
        {
            cx.scene.push_layer();
            let r = Rect::new(b.x, b.y + onto as f32 * ROW, b.w, ROW);
            let x = r.x + self.rows[onto].depth as f32 * INDENT + INDENT;
            match place {
                Place::Into => cx.scene.fill(r.inset(1.0), theme.small_radius(), p.accent.with_alpha(0.18), Some((1.5, p.accent))),
                Place::Before => cx.scene.fill(Rect::new(x, r.y - 1.0, r.right() - x - 2.0, 2.0), 1.0, p.accent, None),
                Place::After => cx.scene.fill(Rect::new(x, r.bottom() - 1.0, r.right() - x - 2.0, 2.0), 1.0, p.accent, None),
            }
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        // A name being edited has the keys and the pointer first.
        if !self.field.is_empty() && Element::event_children(&mut self.field, cx, event) == Status::Captured {
            // Done with, the keyboard comes back to the tree, for the arrow keys to go on from there.
            if matches!(event, Event::Key(k) if k.pressed && matches!(k.key, Key::Enter | Key::Escape)) {
                cx.request_focus();
            }
            return Status::Captured;
        }
        match event {
            Event::PointerMoved { pos } => {
                let over = self.row_at(b, *pos);
                let st = cx.state::<TreeState>();
                st.pointer = *pos;
                if let Some((_, from)) = st.pressed {
                    st.moving |= self.on_move.is_some() && ((pos.x - from.x).abs() > SLACK || (pos.y - from.y).abs() > SLACK);
                    cx.request_redraw();
                    if cx.state::<TreeState>().moving {
                        cx.set_cursor(CursorIcon::Grabbing);
                    }
                    return Status::Captured;
                }
                if st.hovered != over {
                    st.hovered = over;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<TreeState>().hovered = None;
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => {
                let Some(i) = self.row_at(b, *pos) else { return Status::Ignored };
                cx.request_focus();
                let row = &self.rows[i];
                // On the arrow: opened or closed, and nothing else.
                let arrow = b.x + row.depth as f32 * INDENT;
                if row.parent && pos.x >= arrow && pos.x < arrow + INDENT {
                    if let Some(f) = &self.on_toggle {
                        cx.emit(f(row.id.clone(), !row.open));
                    }
                    return Status::Captured;
                }
                let st = cx.state::<TreeState>();
                (st.pressed, st.moving, st.pointer) = (Some((i, *pos)), false, *pos);
                Status::Captured
            }
            Event::PointerReleased { pos, button: PointerButton::Primary } => {
                let st = cx.state::<TreeState>();
                let (Some((i, _)), moving) = (st.pressed.take(), std::mem::take(&mut st.moving)) else { return Status::Ignored };
                cx.request_redraw();
                if moving {
                    if let (Some((onto, place)), Some(f)) = (self.target(b, i, *pos), &self.on_move) {
                        cx.emit(f(self.rows[i].id.clone(), self.rows[onto].id.clone(), place));
                    }
                    return Status::Captured;
                }
                // Clicked: chosen; and clicked again soon after, its name is to be edited.
                let now = cx.now();
                let twice = cx.state::<TreeState>().clicked.is_some_and(|(row, when)| row == i && now.saturating_duration_since(when) < std::time::Duration::from_millis(450));
                cx.state::<TreeState>().clicked = (!twice).then_some((i, now));
                match (twice, &self.on_edit, &self.on_select) {
                    (true, Some(edit), _) => cx.emit(edit(TreeEdit::Begin(self.rows[i].id.clone()))),
                    (_, _, Some(select)) if self.selected.as_ref() != Some(&self.rows[i].id) => cx.emit(select(self.rows[i].id.clone())),
                    _ => {}
                }
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() && self.editing.is_none() => {
                let at = self.selected_row();
                let choose = |cx: &mut EventCx<M>, i: usize| {
                    if let (Some(f), Some(row)) = (&self.on_select, self.rows.get(i)) {
                        cx.emit(f(row.id.clone()));
                    }
                };
                match (&k.key, at) {
                    (Key::Down, _) if !self.rows.is_empty() => choose(cx, at.map_or(0, |i| (i + 1).min(self.rows.len() - 1))),
                    (Key::Up, _) if !self.rows.is_empty() => choose(cx, at.map_or(0, |i| i.saturating_sub(1))),
                    // Right opens, or goes in; left closes, or goes out to what it is inside.
                    (Key::Right, Some(i)) if self.rows[i].parent && !self.rows[i].open => {
                        if let Some(f) = &self.on_toggle {
                            cx.emit(f(self.rows[i].id.clone(), true));
                        }
                    }
                    (Key::Right, Some(i)) if self.rows[i].inside > 0 => choose(cx, i + 1),
                    (Key::Left, Some(i)) if self.rows[i].parent && self.rows[i].open => {
                        if let Some(f) = &self.on_toggle {
                            cx.emit(f(self.rows[i].id.clone(), false));
                        }
                    }
                    (Key::Left, Some(i)) => {
                        if let Some(parent) = (0..i).rev().find(|j| self.rows[*j].depth < self.rows[i].depth) {
                            choose(cx, parent);
                        }
                    }
                    (Key::Enter | Key::F(2), Some(i)) if self.on_edit.is_some() => {
                        if let Some(f) = &self.on_edit {
                            cx.emit(f(TreeEdit::Begin(self.rows[i].id.clone())));
                        }
                    }
                    _ => return Status::Ignored,
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
