//! A number changed by dragging, by the arrow keys and by typing, and a
//! vector's parts each changed on their own.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Event, Key, Modifiers, Point, PointerButton, Size};

#[derive(Clone, Debug)]
enum Msg {
    Speed(f64),
    Place(usize, f64),
    Scrub(bool),
    Pick,
    Clear,
}

struct Inspector {
    speed: f64,
    place: [f64; 3],
    said: u32,
    scrubs: Vec<bool>,
    target: Option<String>,
    picked: u32,
}

impl App for Inspector {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        self.said += 1;
        match m {
            Msg::Speed(v) => self.speed = v,
            Msg::Place(i, v) => self.place[i] = v,
            Msg::Scrub(began) => {
                self.said -= 1;
                self.scrubs.push(began);
            }
            Msg::Pick => self.picked += 1,
            Msg::Clear => self.target = None,
        }
    }

    fn view(&self) -> Element<Msg> {
        column().spacing(10.0).width(300.0).push(number_field(self.speed).step(0.5).range(0.0..=20.0).unit(" m/s").width(120.0).on_change(Msg::Speed).on_scrub(Msg::Scrub)).push(vector_field_scrubbed(&self.place, 0.1, Msg::Place, Msg::Scrub)).push(reference_field(icons::BOX, self.target.as_deref(), Msg::Pick, Some(Msg::Clear))).into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn inspector() -> Harness<Inspector> {
    let mut h = Harness::new(Inspector { speed: 4.0, place: [1.0, 2.0, 3.0], said: 0, scrubs: vec![], target: Some("Player".into()), picked: 0 }, Size::new(320.0, 160.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

fn drag(h: &mut Harness<Inspector>, from: Point, by: f32) {
    h.event(Event::PointerMoved { pos: from });
    h.event(Event::PointerPressed { pos: from, button: PointerButton::Primary });
    h.event(Event::PointerMoved { pos: Point::new(from.x + by / 2.0, from.y + 30.0) });
    h.event(Event::PointerMoved { pos: Point::new(from.x + by, from.y + 30.0) });
    h.event(Event::PointerReleased { pos: Point::new(from.x + by, from.y), button: PointerButton::Primary });
    h.render(1.0);
}

fn key(h: &mut Harness<Inspector>, key: Key) {
    h.key(key, Modifiers::default());
    h.render(1.0);
}

const SPEED: Point = Point { x: 60.0, y: 14.0 };

#[test]
fn a_number_is_dragged_and_nudged_and_kept_within_its_range() {
    let mut h = inspector();
    // Dragged right: a step for every four pixels, said as it goes.
    drag(&mut h, SPEED, 40.0);
    assert_eq!((h.app().speed, h.app().said), (9.0, 2), "ten steps of a half, in two moves");
    assert_eq!(h.app().scrubs, [true, false], "and it is said when the drag began and when it ended");
    // Left, past where it began and on past nought: no lower than it may go.
    drag(&mut h, SPEED, -400.0);
    assert_eq!(h.app().speed, 0.0);
    drag(&mut h, SPEED, 4000.0);
    assert_eq!(h.app().speed, 20.0);
    // A drag leaves it a number to nudge: the arrow keys, a step each.
    key(&mut h, Key::Down);
    key(&mut h, Key::Down);
    key(&mut h, Key::Up);
    assert_eq!(h.app().speed, 19.5);
    // Held where it is, nothing is said.
    let said = h.app().said;
    drag(&mut h, SPEED, 1.0);
    assert_eq!((h.app().speed, h.app().said), (19.5, said));
    assert_eq!(h.app().scrubs.len(), 6, "three drags, and nothing for a nudge or a press that went nowhere");
}

#[test]
fn a_number_is_typed_over() {
    let mut h = inspector();
    // Clicked and not dragged: what is typed takes the number's place, and Enter makes it the number.
    h.click(SPEED);
    h.render(1.0);
    h.type_text("12.5");
    assert_eq!(h.app().speed, 4.0, "not until it is entered");
    key(&mut h, Key::Enter);
    assert_eq!(h.app().speed, 12.5);
    // Enter again to type once more; Backspace clears what was there; Escape leaves it as it was.
    key(&mut h, Key::Enter);
    key(&mut h, Key::Backspace);
    h.type_text("7");
    key(&mut h, Key::Escape);
    assert_eq!(h.app().speed, 12.5);
    // More than it may be is brought within; what is no number changes nothing; letters are not typed at all.
    key(&mut h, Key::Enter);
    h.type_text("950");
    key(&mut h, Key::Enter);
    assert_eq!(h.app().speed, 20.0);
    key(&mut h, Key::Enter);
    h.type_text("fast-.-");
    key(&mut h, Key::Enter);
    assert_eq!(h.app().speed, 20.0);
    // Typed, and then a click elsewhere: entered all the same.
    h.click(SPEED);
    h.type_text("3");
    h.click(Point::new(250.0, 100.0));
    h.render(1.0);
    assert_eq!(h.app().speed, 3.0);
}

#[test]
fn each_part_of_a_vector_is_changed_on_its_own() {
    let mut h = inspector();
    // Three fields share the width under the first: the middle one is Y.
    drag(&mut h, Point::new(150.0, 52.0), 20.0);
    assert_eq!(h.app().place, [1.0, 2.5, 3.0]);
    drag(&mut h, Point::new(260.0, 52.0), -8.0);
    assert_eq!(h.app().place, [1.0, 2.5, 2.8]);
    h.click(Point::new(50.0, 52.0));
    h.type_text("-4.25");
    key(&mut h, Key::Enter);
    assert_eq!(h.app().place, [-4.25, 2.5, 2.8]);
    assert_eq!(h.app().scrubs, [true, false, true, false], "a drag of any part is said, and typing is not one");
}

#[test]
fn a_field_that_stands_for_something_else_is_clicked_to_choose_and_cleared() {
    let mut h = inspector();
    // On its name: another is to be chosen, which is the app's to offer.
    h.click(Point::new(100.0, 90.0));
    assert_eq!((h.app().picked, h.app().target.as_deref()), (1, Some("Player")));
    // The cross at its end clears it, and then there is no cross.
    h.click(Point::new(300.0, 90.0));
    h.render(1.0);
    assert_eq!(h.app().target, None);
    h.click(Point::new(300.0, 90.0));
    assert_eq!((h.app().picked, h.app().target.clone()), (2, None), "all of it is the name now");
}
