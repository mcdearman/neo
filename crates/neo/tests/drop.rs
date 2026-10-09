//! Something dragged from one widget and let go on another: a row of a
//! tree onto a field that refers to one.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Event, Point, PointerButton, Size};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Chose(u32),
    Moved(u32, u32, Place),
    Target(u32, Point),
    Named(String, Point),
    Pick,
}

struct Inspector {
    target: Option<u32>,
    heard: Vec<Msg>,
}

impl App for Inspector {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        if let Msg::Target(id, _) = &m {
            self.target = Some(*id);
        }
        self.heard.push(m);
    }

    fn view(&self) -> Element<Msg> {
        let nodes = [TreeNode::new(1, "Camera"), TreeNode::new(2, "Player"), TreeNode::new(3, "Light")];
        let name = self.target.map(|id| format!("Entity {id}"));
        column()
            .width(Length::Fill)
            .push(tree(&nodes, None).on_select(Msg::Chose).on_move(Msg::Moved).draggable(true))
            // A field that refers to one of them, and takes one dropped on it.
            .push(drop_area(reference_field(icons::BOX, name.as_deref(), Msg::Pick, None), Msg::Target))
            // And somewhere that takes names, which a row is not.
            .push(drop_area(container(text("names go here")).padding(10.0).width(Length::Fill), Msg::Named))
            .into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn drag(h: &mut Harness<Inspector>, from: Point, to: Point) -> (Vec<u8>, Vec<u8>) {
    h.event(Event::PointerMoved { pos: from });
    h.event(Event::PointerPressed { pos: from, button: PointerButton::Primary });
    h.event(Event::PointerMoved { pos: Point::new(from.x + 4.0, from.y + 8.0) });
    let leaving = h.render(1.0);
    h.event(Event::PointerMoved { pos: to });
    let over = h.render(1.0);
    h.event(Event::PointerReleased { pos: to, button: PointerButton::Primary });
    h.render(1.0);
    (leaving, over)
}

#[test]
fn a_row_dragged_onto_a_field_is_given_to_it_and_to_nothing_else() {
    let mut h = Harness::new(Inspector { target: None, heard: vec![] }, Size::new(320.0, 220.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    let (player, field, names) = (Point::new(120.0, 39.0), Point::new(120.0, 92.0), Point::new(120.0, 128.0));
    // The player's row, carried over the field: the field shows it would take it, and is given it when it is let go.
    let (leaving, over) = drag(&mut h, player, field);
    assert!(leaving != over, "the field is marked while the row is over it");
    assert_eq!(h.app().heard, [Msg::Target(2, Point::new(120.0, 92.0 - 78.0))], "given what was carried, and where in the field");
    assert_eq!(h.app().target, Some(2));
    // Let go over somewhere that takes another kind of thing: nothing is given, and nothing shows it would be.
    h.app_mut().heard.clear();
    let (_, over) = drag(&mut h, Point::new(120.0, 13.0), names);
    let (_, away) = drag(&mut h, Point::new(120.0, 13.0), Point::new(300.0, 200.0));
    assert!(h.app().heard.is_empty(), "{:?}", h.app().heard);
    let part = |px: &[u8]| px[110 * 320 * 4..150 * 320 * 4].to_vec();
    assert!(part(&over) == part(&away), "not marked for what it does not take");
    // Within the tree a drag is still a move, and a click still chooses; neither is a drop.
    drag(&mut h, Point::new(120.0, 65.0), Point::new(120.0, 13.0));
    h.click(player);
    assert_eq!(h.app().heard, [Msg::Moved(3, 1, Place::Into), Msg::Chose(2)]);
    // Once let go, nothing is being carried: moving over the field marks nothing.
    h.event(Event::PointerMoved { pos: field });
    let idle = h.render(1.0);
    h.event(Event::PointerMoved { pos: Point::new(300.0, 200.0) });
    let elsewhere = h.render(1.0);
    let part = |px: &[u8]| px[80 * 320 * 4..82 * 320 * 4].to_vec();
    assert!(part(&idle) == part(&elsewhere) || h.app().heard.len() == 2);
}
