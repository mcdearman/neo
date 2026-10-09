//! A colour changed where it stands: opened into a plane of shades and a
//! strip of hues, picked from by dragging, or given as a code.
//!
//! Needs a GPU adapter.

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Color, Event, Key, Modifiers, Point, PointerButton, Size};

#[derive(Clone, Debug)]
enum Msg {
    Tint(Color),
    Scrub(bool),
}

struct Material {
    tint: Color,
    said: u32,
    scrubs: Vec<bool>,
}

impl App for Material {
    type Message = Msg;

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Tint(c) => {
                self.tint = c;
                self.said += 1;
            }
            Msg::Scrub(began) => self.scrubs.push(began),
        }
    }

    fn view(&self) -> Element<Msg> {
        column().width(Length::Fill).push(color_field(self.tint).on_change(Msg::Tint).on_scrub(Msg::Scrub)).push(text("below")).into()
    }

    fn window(&self) -> neo::WindowSettings {
        neo::WindowSettings { decorations: Decorations::System, ..Default::default() }
    }
}

fn material() -> Harness<Material> {
    let mut h = Harness::new(Material { tint: Color::hex(0x3F5BC4), said: 0, scrubs: vec![] }, Size::new(300.0, 260.0)).expect("a GPU adapter is required for these tests");
    h.render(1.0);
    h
}

fn press(h: &mut Harness<Material>, at: Point) {
    h.event(Event::PointerMoved { pos: at });
    h.event(Event::PointerPressed { pos: at, button: PointerButton::Primary });
    h.render(1.0);
}

fn release(h: &mut Harness<Material>, at: Point) {
    h.event(Event::PointerMoved { pos: at });
    h.event(Event::PointerReleased { pos: at, button: PointerButton::Primary });
    h.render(1.0);
}

#[test]
fn a_colour_is_picked_from_its_shades_and_from_the_hues() {
    let mut h = material();
    let shut = h.render(1.0);
    // Shut, there is nothing under it to pick from: a click there is nobody's.
    h.click(Point::new(150.0, 100.0));
    assert_eq!(h.app().said, 0);
    // A click on its swatch opens it, and what was below moves down to make room.
    h.click(Point::new(20.0, 14.0));
    let open = h.render(1.0);
    assert!(shut != open);
    // The top right of the plane is the hue at its fullest; the bottom is black whatever the hue.
    // (Dragged past its edges, the pick stops at them.)
    press(&mut h, Point::new(290.0, 45.0));
    h.event(Event::PointerMoved { pos: Point::new(400.0, -20.0) });
    assert_eq!(hex(h.app().tint), "#0036FF", "its own blue, all colour and all light");
    release(&mut h, Point::new(150.0, 400.0));
    assert_eq!(hex(h.app().tint), "#000000");
    assert_eq!(h.app().scrubs, [true, false], "one drag, from the press to the letting go");
    // Black has no hue of its own, but the picking has not forgotten which it was on:
    // back at the top right it is that blue again.
    press(&mut h, Point::new(290.0, 45.0));
    release(&mut h, Point::new(400.0, -20.0));
    assert_eq!(hex(h.app().tint), "#0036FF");
    // Along the strip, a third of the way: green, at the same fullness.
    press(&mut h, Point::new(100.0, 170.0));
    release(&mut h, Point::new(100.0, 170.0));
    assert_eq!(hex(h.app().tint), "#00FF00");
    // A click on the swatch shuts it again.
    h.click(Point::new(20.0, 14.0));
    assert!(h.render(1.0) != open);
    let said = h.app().said;
    h.click(Point::new(150.0, 100.0));
    assert_eq!(h.app().said, said);
}

#[test]
fn a_colour_is_given_as_its_code() {
    let mut h = material();
    h.click(Point::new(20.0, 14.0));
    // Open, a click on the code types over it; what is typed is the colour once entered.
    h.click(Point::new(100.0, 14.0));
    h.type_text("#E0569B");
    assert_eq!(hex(h.app().tint), "#3F5BC4", "not until it is entered");
    h.key(Key::Enter, Modifiers::default());
    h.render(1.0);
    assert_eq!((hex(h.app().tint), h.app().scrubs.len()), ("#E0569B".to_owned(), 0));
    // What is no colour changes nothing, and neither does Escape; a click elsewhere enters what is typed.
    h.click(Point::new(100.0, 14.0));
    h.type_text("12");
    h.key(Key::Enter, Modifiers::default());
    h.click(Point::new(100.0, 14.0));
    h.type_text("000000");
    h.key(Key::Escape, Modifiers::default());
    assert_eq!(hex(h.app().tint), "#E0569B");
    h.click(Point::new(100.0, 14.0));
    h.type_text("fff");
    h.click(Point::new(150.0, 240.0));
    h.render(1.0);
    assert_eq!(hex(h.app().tint), "#FFFFFF");
    // Changed from elsewhere, it shows what it is now and picks from there.
    h.app_mut().tint = Color::hex(0xFF0000);
    h.render(1.0);
    press(&mut h, Point::new(10.0, 45.0));
    release(&mut h, Point::new(-50.0, -20.0));
    assert_eq!(hex(h.app().tint), "#FFFFFF", "the top left of any hue's plane is white");
    press(&mut h, Point::new(290.0, 45.0));
    release(&mut h, Point::new(400.0, -20.0));
    assert_eq!(hex(h.app().tint), "#FF0000", "and its top right is the red it was given");
}
