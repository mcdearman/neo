//! A tree of things inside things: rows opened and closed, chosen with
//! the pointer and the keys, renamed where they stand, and dragged into
//! one another.
//!
//! Needs a GPU adapter.

use std::time::Duration;

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Event, Key, Modifiers, Point, PointerButton, Size};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Chose(u32),
    Toggled(u32, bool),
    Moved(u32, u32, Place),
    Edit(TreeEdit<u32>),
}

/// A scene: a world with a camera, a player holding a sword, and a light.
struct Scene {
    open: Vec<u32>,
    chosen: Option<u32>,
    editing: Option<u32>,
    typed: String,
    names: Vec<(u32, String)>,
    said: Vec<Msg>,
}

impl Scene {
    fn name(&self, id: u32) -> String {
        self.names.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()).unwrap_or_default()
    }

    fn node(&self, id: u32, children: Vec<TreeNode<u32>>) -> TreeNode<u32> {
        TreeNode::new(id, self.name(id)).with(self.open.contains(&id), children)
    }

    fn nodes(&self) -> Vec<TreeNode<u32>> {
        vec![self.node(1, vec![self.node(2, vec![]), self.node(3, vec![self.node(4, vec![])]), self.node(5, vec![])]), self.node(6, vec![])]
    }
}

impl App for Scene {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        self.said.push(m.clone());
        match m {
            Msg::Chose(id) => self.chosen = Some(id),
            Msg::Toggled(id, true) => self.open.push(id),
            Msg::Toggled(id, false) => self.open.retain(|o| *o != id),
            Msg::Moved(..) => {}
            Msg::Edit(TreeEdit::Begin(id)) => (self.editing, self.typed) = (Some(id), self.name(id)),
            Msg::Edit(TreeEdit::Typed(t)) => self.typed = t,
            Msg::Edit(TreeEdit::Done) => {
                if let (Some(id), typed) = (self.editing.take(), std::mem::take(&mut self.typed)) {
                    self.names.iter_mut().filter(|(i, _)| *i == id).for_each(|(_, n)| *n = typed.clone());
                }
            }
            Msg::Edit(TreeEdit::Dropped) => self.editing = None,
        }
    }

    fn view(&self) -> Element<Msg> {
        tree(&self.nodes(), self.chosen.as_ref()).on_select(Msg::Chose).on_toggle(Msg::Toggled).on_move(Msg::Moved).on_edit(Msg::Edit).editing(self.editing.as_ref(), &self.typed).into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn scene() -> Harness<Scene> {
    let names = [(1, "World"), (2, "Camera"), (3, "Player"), (4, "Sword"), (5, "Light"), (6, "Sky")].map(|(i, n)| (i, n.to_owned())).to_vec();
    let mut h = Harness::new(Scene { open: vec![1], chosen: None, editing: None, typed: String::new(), names, said: vec![] }, Size::new(300.0, 400.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

/// The middle of row `i`, on its name.
fn row(i: usize) -> Point {
    Point::new(120.0, i as f32 * 26.0 + 13.0)
}

fn key(h: &mut Harness<Scene>, key: Key) {
    h.key(key, Modifiers::default());
    h.render(1.0);
}

#[test]
fn rows_are_opened_and_closed_and_chosen_with_the_pointer_and_the_keys() {
    let mut h = scene();
    // World is open: itself, its three, and Sky. Player is shut, so no Sword.
    h.click(row(2));
    h.render(1.0);
    assert_eq!(h.app().chosen, Some(3), "the third row is Player");
    // A click on a row's arrow opens it, and chooses nothing.
    h.click(Point::new(16.0 + 8.0, row(2).y));
    h.render(1.0);
    assert_eq!((h.app().open.clone(), h.app().chosen), (vec![1, 3], Some(3)));
    h.click(row(3));
    h.render(1.0);
    assert_eq!(h.app().chosen, Some(4), "and what was inside it is a row now");
    // The keys: down and up along the rows, left out to what a row is inside, and to shut it, right to open and go in.
    key(&mut h, Key::Down);
    assert_eq!(h.app().chosen, Some(5));
    key(&mut h, Key::Up);
    key(&mut h, Key::Left);
    assert_eq!(h.app().chosen, Some(3), "out of the sword to the player");
    key(&mut h, Key::Left);
    assert_eq!((h.app().open.clone(), h.app().chosen), (vec![1], Some(3)), "shut");
    key(&mut h, Key::Right);
    key(&mut h, Key::Right);
    assert_eq!((h.app().open.clone(), h.app().chosen), (vec![1, 3], Some(4)), "opened, and then in");
    for _ in 0..9 {
        key(&mut h, Key::Down);
    }
    assert_eq!(h.app().chosen, Some(6), "no further than the last");
    // One already chosen, clicked: nothing more is said of it.
    let said = h.app().said.len();
    h.advance(Duration::from_secs(2));
    h.click(row(5));
    assert_eq!(h.app().said.len(), said);
}

#[test]
fn a_name_is_edited_where_it_stands() {
    let mut h = scene();
    // Clicked twice quickly: its name is to be edited, and the app says it is.
    h.click(row(1));
    h.click(row(1));
    h.render(1.0);
    assert_eq!((h.app().editing, h.app().typed.as_str()), (Some(2), "Camera"));
    // What is typed is the app's as it is typed, and Enter makes it the name.
    // The old name is all selected, as a name to be replaced is: typing takes its place.
    h.type_text("Lens");
    assert_eq!(h.app().typed, "Lens");
    key(&mut h, Key::Enter);
    assert_eq!((h.app().editing, h.app().name(2)), (None, "Lens".to_owned()));
    // From the keys, with the row chosen; Escape leaves the name as it was.
    key(&mut h, Key::F(2));
    assert_eq!(h.app().editing, Some(2));
    h.type_text("zzz");
    key(&mut h, Key::Escape);
    assert_eq!((h.app().editing, h.app().name(2)), (None, "Lens".to_owned()));
    // While a name is edited the arrow keys are the field's, not the tree's.
    key(&mut h, Key::Enter);
    key(&mut h, Key::Down);
    assert_eq!((h.app().editing, h.app().chosen), (Some(2), Some(2)));
}

#[test]
fn a_row_is_dragged_into_another_or_between_two() {
    let mut h = scene();
    let drag = |h: &mut Harness<Scene>, from: Point, to: Point| {
        h.event(Event::PointerMoved { pos: from });
        h.event(Event::PointerPressed { pos: from, button: PointerButton::Primary });
        h.event(Event::PointerMoved { pos: Point::new(from.x + 3.0, (from.y + to.y) / 2.0) });
        h.render(1.0);
        h.event(Event::PointerMoved { pos: to });
        h.event(Event::PointerReleased { pos: to, button: PointerButton::Primary });
        h.render(1.0);
    };
    // The light, onto the middle of the camera: into it.
    drag(&mut h, row(3), row(1));
    assert_eq!(h.app().said.last(), Some(&Msg::Moved(5, 2, Place::Into)));
    // Onto the top edge of the camera: before it; the bottom edge of the sky: after it.
    drag(&mut h, row(3), Point::new(120.0, 26.0 + 3.0));
    assert_eq!(h.app().said.last(), Some(&Msg::Moved(5, 2, Place::Before)));
    drag(&mut h, row(1), Point::new(120.0, 4.0 * 26.0 + 24.0));
    assert_eq!(h.app().said.last(), Some(&Msg::Moved(2, 6, Place::After)));
    // The world onto something inside itself, or onto itself, or off the tree: nowhere.
    let said = h.app().said.len();
    drag(&mut h, row(0), row(2));
    drag(&mut h, row(0), Point::new(120.0, 20.0));
    drag(&mut h, row(1), Point::new(120.0, 390.0));
    assert_eq!(h.app().said.len(), said);
    // And a drag is not a click: nothing was chosen by any of it.
    assert_eq!(h.app().chosen, None);
}
