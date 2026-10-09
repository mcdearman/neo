//! Panels arranged in a window and rearranged by hand: tabs chosen, the
//! bars between groups dragged, a tab dragged to another place.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Event, Point, PointerButton, Size};

#[derive(Clone, Debug)]
enum Msg {
    Arranged(Dock),
    Pressed(String),
}

struct Editor {
    layout: Dock,
    pressed: Vec<String>,
}

impl App for Editor {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Arranged(layout) => self.layout = layout,
            Msg::Pressed(panel) => self.pressed.push(panel),
        }
    }

    fn view(&self) -> Element<Msg> {
        // Each panel is a button as large as the panel, that says which it is.
        dock(&self.layout, |panel| panel.to_uppercase(), |panel| mouse_area(container(text(panel.to_owned())).width(Length::Fill).height(Length::Fill))
            .on_press({
                let panel = panel.to_owned();
                move || Msg::Pressed(panel.clone())
            })
            .into(), Msg::Arranged).into()
    }

    // No title bar of Neo's own: the dock has the whole window, from its corner.
    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn editor() -> Harness<Editor> {
    let layout = Dock::beside(Dock::tabs(["scene", "assets"]), 0.25, Dock::above(Dock::tabs(["view"]), 0.7, Dock::tabs(["console", "log"])));
    let mut h = Harness::new(Editor { layout, pressed: vec![] }, Size::new(800.0, 600.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

fn press(h: &mut Harness<Editor>, at: Point) {
    h.event(Event::PointerMoved { pos: at });
    h.event(Event::PointerPressed { pos: at, button: PointerButton::Primary });
}

fn release(h: &mut Harness<Editor>, at: Point) {
    h.event(Event::PointerMoved { pos: at });
    h.event(Event::PointerReleased { pos: at, button: PointerButton::Primary });
    h.render(1.0);
}

/// Where along a strip of tabs at height `y`, from `x`, a click brings `panel` to the front.
fn tab(h: &mut Harness<Editor>, panel: &str, x: f32, y: f32) -> Point {
    (0..60)
        .map(|i| Point::new(x + 6.0 + i as f32 * 6.0, y))
        .find(|at| {
            h.click(*at);
            h.render(1.0);
            h.app().layout.shown().contains(&panel)
        })
        .unwrap_or_else(|| panic!("no tab for {panel}"))
}

#[test]
fn tabs_are_chosen_and_what_is_in_a_panel_still_hears_the_pointer() {
    let mut h = editor();
    assert_eq!(h.app().layout.shown(), ["scene", "view", "console"]);
    // A click in a panel is the panel's.
    h.click(Point::new(100.0, 300.0));
    h.click(Point::new(500.0, 200.0));
    h.click(Point::new(500.0, 520.0));
    assert_eq!(h.app().pressed, ["scene", "view", "console"]);
    // A click on a tab brings its panel forward, and then that one is what is clicked in.
    let assets = tab(&mut h, "assets", 0.0, 15.0);
    assert!(assets.x > 30.0 && assets.x < 199.0, "beside the first tab, in its own group: {assets:?}");
    assert_eq!(h.app().layout.encode(), "row(0.25, tabs(scene, *assets), column(0.7, tabs(*view), tabs(*console, log)))");
    h.click(Point::new(100.0, 300.0));
    assert_eq!(h.app().pressed.last().map(String::as_str), Some("assets"));
    // The tab already in front: nothing changes, and nothing is said.
    let before = h.app().layout.clone();
    h.click(assets);
    assert_eq!(h.app().layout, before);
}

#[test]
fn the_bar_between_two_groups_is_dragged_to_share_the_room_differently() {
    let mut h = editor();
    // The upright bar is a quarter of the way across; dragged right, the first group has more.
    press(&mut h, Point::new(201.0, 300.0));
    h.event(Event::PointerMoved { pos: Point::new(321.0, 310.0) });
    release(&mut h, Point::new(321.0, 310.0));
    let Dock::Split { share, .. } = h.app().layout.clone() else { panic!("a split") };
    assert!((share - 0.4).abs() < 0.01, "{share}");
    assert!(h.app().pressed.is_empty(), "a drag of the bar is not a click in a panel");
    // What was the second group's is the first's now.
    h.click(Point::new(300.0, 300.0));
    assert_eq!(h.app().pressed, ["scene"]);
    // The level bar, in the second group: dragged up as far as it will go, it stops short.
    press(&mut h, Point::new(600.0, 419.0));
    release(&mut h, Point::new(600.0, -200.0));
    assert!(h.app().layout.encode().contains("column(0.08, "), "{}", h.app().layout.encode());
}

#[test]
fn a_tab_is_dragged_to_another_group_or_to_one_side_of_it() {
    let mut h = editor();
    let log = tab(&mut h, "log", 204.0, 437.0);
    // Pressed and let go where it is: a click, as before.
    assert_eq!(h.app().layout.encode(), "row(0.25, tabs(*scene, assets), column(0.7, tabs(*view), tabs(console, *log)))");
    // Dragged to the right of the group above: a group of its own there.
    press(&mut h, log);
    h.event(Event::PointerMoved { pos: Point::new(log.x + 40.0, 300.0) });
    h.render(1.0);
    release(&mut h, Point::new(700.0, 200.0));
    assert_eq!(h.app().layout.encode(), "row(0.25, tabs(*scene, assets), column(0.7, row(0.5, tabs(*view), tabs(*log)), tabs(*console)))");
    h.click(Point::new(700.0, 200.0));
    assert_eq!(h.app().pressed.last().map(String::as_str), Some("log"), "and it is shown there");
    // Dragged onto another group's tabs: one of them, and in front.
    let scene = tab(&mut h, "scene", 0.0, 15.0);
    press(&mut h, scene);
    release(&mut h, Point::new(500.0, 520.0));
    assert_eq!(h.app().layout.encode(), "row(0.25, tabs(*assets), column(0.7, row(0.5, tabs(*view), tabs(*log)), tabs(console, *scene)))");
    // Dragged off the window altogether: left where it was.
    let before = h.app().layout.clone();
    let assets = tab(&mut h, "assets", 0.0, 15.0);
    press(&mut h, assets);
    release(&mut h, Point::new(-50.0, 900.0));
    assert_eq!(h.app().layout, before);
}

/// A picture of it, to look at by hand:
/// `NEO_DOCK_PNG=/tmp/dock.png cargo test -p neo --test dock a_picture -- --ignored`.
#[test]
#[ignore]
fn a_picture() {
    let mut h = editor();
    // With a tab picked up, to show where it would land.
    let log = tab(&mut h, "log", 204.0, 437.0);
    press(&mut h, log);
    h.event(Event::PointerMoved { pos: Point::new(700.0, 200.0) });
    h.save_png(std::env::var("NEO_DOCK_PNG").expect("NEO_DOCK_PNG"), 2.0).unwrap();
}
