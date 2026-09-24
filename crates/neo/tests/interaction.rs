//! End-to-end behaviour through the headless harness. Needs a GPU adapter
//! (any Metal, Vulkan, DX12 or GL device, including software rasterisers).

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Key, Modifiers, Point, Size};

#[derive(Default)]
struct Form {
    clicks: u32,
    name: String,
    on: bool,
    level: f32,
    submitted: bool,
}

#[derive(Clone, Debug)]
enum Msg {
    Click,
    Name(String),
    Toggle(bool),
    Level(f32),
    Submit,
}

impl App for Form {
    type Message = Msg;

    fn window(&self) -> WindowSettings {
        WindowSettings { decorations: Decorations::System, ..Default::default() }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Click => self.clicks += 1,
            Msg::Name(n) => self.name = n,
            Msg::Toggle(b) => self.on = b,
            Msg::Level(l) => self.level = l,
            Msg::Submit => self.submitted = true,
        }
    }

    // Layout: 20px padding, children stacked with 10px spacing.
    fn view(&self) -> Element<Msg> {
        column()
            .padding(20.0)
            .spacing(10.0)
            .push(button("Press").on_press(Msg::Click).width(120.0).height(40.0))
            .push(text_input("Name", self.name.clone()).on_input(Msg::Name).on_submit(Msg::Submit).width(300.0))
            .push(toggle(self.on, Msg::Toggle))
            .push(container(slider(0.0..=10.0, self.level, Msg::Level).step(1.0)).width(300.0))
            .into()
    }
}

fn harness() -> Harness<Form> {
    Harness::new(Form::default(), Size::new(400.0, 300.0)).expect("a GPU adapter is required for these tests")
}

#[test]
fn clicking_a_button_sends_its_message() {
    let mut h = harness();
    h.click(Point::new(60.0, 40.0));
    h.click(Point::new(60.0, 40.0));
    assert_eq!(h.app().clicks, 2);
    // A press that ends outside the button does not count.
    h.event(neo::Event::PointerPressed { pos: Point::new(60.0, 40.0), button: neo::PointerButton::Primary });
    h.event(neo::Event::PointerReleased { pos: Point::new(390.0, 290.0), button: neo::PointerButton::Primary });
    assert_eq!(h.app().clicks, 2);
}

#[test]
fn typing_edits_the_text_input() {
    let mut h = harness();
    h.click(Point::new(100.0, 90.0));
    h.type_text("Ada Lovelace");
    assert_eq!(h.app().name, "Ada Lovelace");
    h.key(Key::Backspace, Modifiers::default());
    assert_eq!(h.app().name, "Ada Lovelac");
    let cmd = if cfg!(target_os = "macos") { Modifiers { logo: true, ..Default::default() } } else { Modifiers { ctrl: true, ..Default::default() } };
    h.key(Key::Character("a".into()), cmd);
    h.type_text("Grace");
    assert_eq!(h.app().name, "Grace");
    h.key(Key::Enter, Modifiers::default());
    assert!(h.app().submitted);
}

#[test]
fn tab_moves_focus_and_keys_operate_controls() {
    let mut h = harness();
    // Button, text input, toggle, slider.
    h.key(Key::Tab, Modifiers::default());
    h.key(Key::Space, Modifiers::default());
    assert_eq!(h.app().clicks, 1);
    h.key(Key::Tab, Modifiers::default());
    h.key(Key::Tab, Modifiers::default());
    h.key(Key::Space, Modifiers::default());
    assert!(h.app().on);
    h.key(Key::Tab, Modifiers::default());
    h.key(Key::Right, Modifiers::default());
    h.key(Key::Right, Modifiers::default());
    assert_eq!(h.app().level, 2.0);
    h.key(Key::End, Modifiers::default());
    assert_eq!(h.app().level, 10.0);
    // Shift+Tab goes back to the toggle.
    h.key(Key::Tab, Modifiers { shift: true, ..Default::default() });
    h.key(Key::Space, Modifiers::default());
    assert!(!h.app().on);
}

#[test]
fn dragging_the_slider_snaps_to_steps() {
    let mut h = harness();
    // Slider row: y = 20 + 40 + 10 + input + 10 + 28 + 10; find it by scanning.
    let y = (150..260).find(|y| {
        h.click(Point::new(300.0, *y as f32));
        h.app().level > 0.0
    });
    assert!(y.is_some(), "slider should respond to a click near its right end");
    assert_eq!(h.app().level.fract(), 0.0);
}

#[test]
fn renders_every_style_without_panicking() {
    for style in [Style::Flat, Style::Soft] {
        for scheme in [Scheme::Light, Scheme::Dark] {
            struct One(Style, Scheme);
            impl App for One {
                type Message = ();
                fn update(&mut self, _: ()) {}
                fn theme(&self, _: Scheme) -> Theme {
                    Theme { style: self.0, scheme: self.1, ..Theme::default() }
                }
                fn view(&self) -> Element<()> {
                    column().push(button("A")).push(gauge(0.5, "50%")).push(sparkline(vec![1.0, 3.0, 2.0], 0.0, 4.0)).into()
                }
            }
            let mut h = Harness::new(One(style, scheme), Size::new(200.0, 300.0)).unwrap();
            let px = h.render(1.0);
            let bg = Theme { style, scheme, ..Theme::default() }.palette().bg.to_rgba8();
            // A pixel inside the window but away from widgets is the window background.
            let i = ((290 * 200 + 190) * 4) as usize;
            let got = &px[i..i + 4];
            let close = got.iter().zip(bg.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 2);
            assert!(close, "{style:?}/{scheme:?}: expected background {bg:?}, got {got:?}");
        }
    }
}

mod layout {
    use super::*;

    struct Split;
    impl App for Split {
        type Message = ();
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, _: ()) {}
        fn view(&self) -> Element<()> {
            // 400px wide: a fixed 100px, then portions 1:3 of the remaining 300.
            row()
                .width(Length::Fill)
                .push(container(Space::new(0.0, 10.0)).width(100.0).background(Background::Color(Color::hex(0xff0000))))
                .push(container(Space::new(0.0, 10.0)).width(Length::Portion(1)).background(Background::Color(Color::hex(0x00ff00))))
                .push(container(Space::new(0.0, 10.0)).width(Length::Portion(3)).background(Background::Color(Color::hex(0x0000ff))))
                .into()
        }
    }

    #[test]
    fn row_shares_space_by_portion() {
        let mut h = Harness::new(Split, Size::new(400.0, 20.0)).unwrap();
        let px = h.render(1.0);
        let at = |x: usize| {
            let i = (5 * 400 + x) * 4;
            [px[i], px[i + 1], px[i + 2]]
        };
        assert_eq!(at(50), [255, 0, 0]);
        assert_eq!(at(99), [255, 0, 0]);
        assert_eq!(at(100), [0, 255, 0]);
        assert_eq!(at(174), [0, 255, 0]);
        assert_eq!(at(175), [0, 0, 255]);
        assert_eq!(at(399), [0, 0, 255]);
    }
}

mod scrolling {
    use super::*;

    struct List;
    impl App for List {
        type Message = u32;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, _: u32) {}
        fn view(&self) -> Element<u32> {
            let col = (0..50).fold(column(), |c, i| c.push(container(text(format!("Row {i}"))).height(20.0)));
            scrollable(col).into()
        }
    }

    #[test]
    fn wheel_scrolls_and_clamps() {
        let mut h = Harness::new(List, Size::new(200.0, 100.0)).unwrap();
        let before = h.render(1.0);
        h.event(neo::Event::Wheel { pos: Point::new(50.0, 50.0), delta: Point::new(0.0, 60.0) });
        let after = h.render(1.0);
        assert_ne!(before, after, "content should move after scrolling");
        // Scrolling far past the end clamps to the last page rather than blanking.
        h.event(neo::Event::Wheel { pos: Point::new(50.0, 50.0), delta: Point::new(0.0, 10_000.0) });
        let end = h.render(1.0);
        h.event(neo::Event::Wheel { pos: Point::new(50.0, 50.0), delta: Point::new(0.0, 100.0) });
        assert_eq!(end, h.render(1.0));
    }
}

mod editor {
    use super::*;
    use neo::widgets::{Action, Document};

    struct Ed {
        doc: Document,
    }

    impl App for Ed {
        type Message = Action;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, a: Action) {
            self.doc.apply(a);
        }
        fn view(&self) -> Element<Action> {
            text_editor(&self.doc).language(Language::Rust).on_action(|a| a).into()
        }
    }

    fn cmd() -> Modifiers {
        if cfg!(target_os = "macos") { Modifiers { logo: true, ..Default::default() } } else { Modifiers { ctrl: true, ..Default::default() } }
    }

    #[test]
    fn typing_editing_and_undo_through_the_widget() {
        let mut h = Harness::new(Ed { doc: Document::new("fn main() {}") }, Size::new(600.0, 300.0)).unwrap();
        // Click far right of the first line: cursor goes to the end.
        h.click(Point::new(590.0, 20.0));
        assert_eq!(h.app().doc.cursor().col, 12);
        h.key(Key::Left, Modifiers::default());
        h.key(Key::Enter, Modifiers::default());
        h.type_text("run();");
        assert_eq!(h.app().doc.text(), "fn main() {\n    run();\n}");
        h.key(Key::Character("z".into()), cmd());
        assert_eq!(h.app().doc.text(), "fn main() {\n    \n}");
        h.key(Key::Character("z".into()), Modifiers { shift: true, ..cmd() });
        assert_eq!(h.app().doc.text(), "fn main() {\n    run();\n}");
        // Tab indents, Shift+Tab outdents.
        h.key(Key::Home, Modifiers::default());
        h.key(Key::Tab, Modifiers::default());
        assert_eq!(h.app().doc.lines()[1], "        run();");
        h.key(Key::Tab, Modifiers { shift: true, ..Default::default() });
        assert_eq!(h.app().doc.lines()[1], "    run();");
    }

    #[test]
    fn select_all_cut_and_paste() {
        let mut h = Harness::new(Ed { doc: Document::new("one\ntwo") }, Size::new(400.0, 200.0)).unwrap();
        h.click(Point::new(200.0, 20.0));
        h.key(Key::Character("a".into()), cmd());
        h.key(Key::Character("x".into()), cmd());
        assert_eq!(h.app().doc.text(), "");
        // The harness has no system clipboard, so paste text arrives through the runtime.
        h.paste("three\nfour", cmd());
        assert_eq!(h.app().doc.text(), "three\nfour");
    }
}
