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
fn renders_every_scheme_without_panicking() {
    {
        for scheme in [Scheme::Light, Scheme::Dark] {
            struct One(Scheme);
            impl App for One {
                type Message = ();
                fn update(&mut self, _: ()) {}
                fn theme(&self, _: Scheme) -> Theme {
                    Theme { scheme: self.0, ..Theme::default() }
                }
                fn view(&self) -> Element<()> {
                    column().push(button("A")).push(gauge(0.5, "50%")).push(sparkline(vec![1.0, 3.0, 2.0], 0.0, 4.0)).into()
                }
            }
            let mut h = Harness::new(One(scheme), Size::new(200.0, 300.0)).unwrap();
            let px = h.render(1.0);
            let bg = Theme { scheme, ..Theme::default() }.palette().bg.to_rgba8();
            // A pixel inside the window but away from widgets is the window background.
            let i = ((290 * 200 + 190) * 4) as usize;
            let got = &px[i..i + 4];
            let close = got.iter().zip(bg.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 2);
            assert!(close, "{scheme:?}: expected background {bg:?}, got {got:?}");
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

mod vim_editor {
    use super::*;
    use neo::widgets::{Action, Document, VimMode, VimRequest};

    struct Ed {
        doc: Document,
        requests: Vec<VimRequest>,
    }

    impl App for Ed {
        type Message = Action;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, a: Action) {
            self.doc.apply(a);
            self.requests.extend(self.doc.take_vim_requests());
        }
        fn view(&self) -> Element<Action> {
            text_editor(&self.doc).on_action(|a| a).into()
        }
    }

    #[test]
    fn vim_keys_flow_through_the_widget() {
        let mut doc = Document::new("alpha beta\ngamma");
        doc.set_vim(true);
        let mut h = Harness::new(Ed { doc, requests: vec![] }, Size::new(600.0, 300.0)).unwrap();
        h.click(Point::new(80.0, 20.0));
        h.key(Key::Character("0".into()), Modifiers::default());
        h.type_text("dw");
        assert_eq!(h.app().doc.text(), "beta\ngamma");
        h.type_text("ciwdelta");
        assert_eq!(h.app().doc.vim().unwrap().mode, VimMode::Insert);
        h.key(Key::Escape, Modifiers::default());
        assert_eq!(h.app().doc.text(), "delta\ngamma");
        assert_eq!(h.app().doc.vim().unwrap().mode, VimMode::Normal);
        h.type_text("vey");
        assert_eq!(h.app().doc.vim().unwrap().mode, VimMode::Normal);
        h.type_text(":w");
        h.key(Key::Enter, Modifiers::default());
        assert_eq!(h.app().requests, vec![VimRequest::Write]);
        // Ctrl-R redoes in Vim mode instead of going to the application.
        h.type_text("u");
        assert_eq!(h.app().doc.text(), "beta\ngamma");
        h.key(Key::Character("r".into()), Modifiers { ctrl: true, ..Default::default() });
        assert_eq!(h.app().doc.text(), "delta\ngamma");
    }
}

mod vim_clipboard {
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
            text_editor(&self.doc).on_action(|a| a).into()
        }
    }

    #[test]
    fn plus_register_uses_the_system_clipboard() {
        let mut doc = Document::new("first line\nsecond");
        doc.set_vim(true);
        let mut h = Harness::new(Ed { doc }, Size::new(600.0, 300.0)).unwrap();
        h.click(Point::new(40.0, 20.0));
        h.type_text("\"+yy");
        assert_eq!(h.clipboard().as_deref(), Some("first line\n"));
        h.set_clipboard("pasted");
        h.type_text("j0\"+P");
        assert_eq!(h.app().doc.lines()[1], "pastedsecond");
        // With clipboard=unnamedplus, plain y and p use it too.
        h.type_text(":set clipboard=unnamedplus");
        h.key(Key::Enter, Modifiers::default());
        h.set_clipboard("X");
        h.type_text("0p");
        assert_eq!(h.app().doc.lines()[1], "pXastedsecond");
        h.type_text("yiw");
        assert_eq!(h.clipboard().as_deref(), Some("pXastedsecond"));
    }
}

mod runtime {
    use super::*;
    use neo::{Cx, DrawCx, Limits, Proxy, Widget};
    use std::time::Duration;

    /// Receives a message from another thread through its proxy.
    #[derive(Default)]
    struct Worker {
        got: Vec<u32>,
    }

    impl App for Worker {
        type Message = u32;
        fn start(&mut self, proxy: Proxy<u32>) {
            std::thread::spawn(move || {
                proxy.send(7);
                proxy.send(8);
            });
        }
        fn update(&mut self, m: u32) {
            self.got.push(m);
        }
        fn view(&self) -> Element<u32> {
            text(format!("{:?}", self.got)).into()
        }
    }

    #[test]
    fn proxy_delivers_messages_from_other_threads() {
        let mut h = Harness::new(Worker::default(), Size::new(200.0, 100.0)).unwrap();
        for _ in 0..100 {
            if h.app().got.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            h.advance(Duration::from_millis(5));
        }
        assert_eq!(h.app().got, [7, 8]);
    }

    /// A widget that reports its width during layout.
    struct Measure;

    impl Widget<f32> for Measure {
        fn width(&self) -> Length {
            Length::Fill
        }
        fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
            cx.defer(limits.max.w);
            Size::new(limits.max.w, 10.0)
        }
        fn draw(&self, _cx: &mut DrawCx) {}
    }

    #[derive(Default)]
    struct Sized {
        width: f32,
    }

    impl App for Sized {
        type Message = f32;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, w: f32) {
            self.width = w;
        }
        fn view(&self) -> Element<f32> {
            Element::new(Measure)
        }
    }

    #[test]
    fn deferred_messages_reach_the_app_after_layout() {
        let mut h = Harness::new(Sized::default(), Size::new(300.0, 100.0)).unwrap();
        h.render(1.0);
        assert_eq!(h.app().width, 300.0);
        h.resize(Size::new(420.0, 100.0));
        h.render(1.0);
        assert_eq!(h.app().width, 420.0);
    }
}

mod pictures_and_menus {
    use super::*;
    use neo::Image;

    struct Show(Image);

    impl App for Show {
        type Message = ();
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, _: ()) {}
        fn view(&self) -> Element<()> {
            // The left half of the picture over the whole window, with a label on top.
            stack().width(Length::Fill).height(Length::Fill).push(picture(&self.0).fit(Fit::Fill).width(Length::Fill).height(Length::Fill)).push(text("over")).into()
        }
    }

    #[test]
    fn pictures_are_drawn_with_their_own_colours() {
        // Left half red, right half blue, half see-through at the bottom.
        let mut px = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let a = if y < 2 { 255 } else { 128 };
                px.extend_from_slice(if x < 2 { &[255, 0, 0] } else { &[0, 0, 255] });
                px.push(a);
            }
        }
        let mut h = Harness::new(Show(Image::new(4, 4, px)), Size::new(200.0, 200.0)).unwrap();
        let out = h.render(1.0);
        let at = |x: usize, y: usize| -> [u8; 4] { out[(y * 200 + x) * 4..][..4].try_into().unwrap() };
        let near = |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(p, q)| (*p as i32 - q as i32).abs() <= 6);
        assert!(near(at(30, 30), [255, 0, 0, 255]), "top left is red: {:?}", at(30, 30));
        assert!(near(at(170, 30), [0, 0, 255, 255]), "top right is blue: {:?}", at(170, 30));
        // The see-through half lets the window background show through.
        let bottom = at(30, 180);
        assert!(bottom[0] > 200 && bottom[1] > 60 && bottom[1] < 160, "bottom left is red over the background: {bottom:?}");
    }

    #[derive(Default)]
    struct Chooser {
        open: bool,
        chosen: Option<&'static str>,
        at: Point,
    }

    #[derive(Clone)]
    enum Pick {
        Menu(Point),
        Choose(&'static str),
        Dismiss,
    }

    impl App for Chooser {
        type Message = Pick;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, m: Pick) {
            match m {
                Pick::Menu(p) => {
                    self.open = true;
                    self.at = p;
                }
                Pick::Choose(what) => {
                    self.chosen = Some(what);
                    self.open = false;
                }
                Pick::Dismiss => self.open = false,
            }
        }
        fn view(&self) -> Element<Pick> {
            let base = mouse_area(container(text("right-click me")).width(Length::Fill).height(Length::Fill)).on_secondary_press(Pick::Menu);
            let mut s = stack().width(Length::Fill).height(Length::Fill).push(base);
            if self.open {
                s = s.push(popup_menu(self.at, vec![MenuItem::new("Open", Pick::Choose("open")), MenuItem::separator(), MenuItem::disabled("Paste"), MenuItem::new("Delete", Pick::Choose("delete")).danger()], Pick::Dismiss));
            }
            s.into()
        }
    }

    fn right_click(h: &mut Harness<Chooser>, p: Point) {
        h.move_to(p);
        h.event(neo::Event::PointerPressed { pos: p, button: neo::PointerButton::Secondary });
        h.event(neo::Event::PointerReleased { pos: p, button: neo::PointerButton::Secondary });
    }

    #[test]
    fn a_right_click_opens_a_menu_that_chooses_or_dismisses() {
        let mut h = Harness::new(Chooser::default(), Size::new(400.0, 300.0)).unwrap();
        right_click(&mut h, Point::new(100.0, 80.0));
        assert!(h.app().open);
        // Rows are 32 high after 6 of padding: Open, a separator, Paste, Delete.
        h.click(Point::new(140.0, 80.0 + 6.0 + 32.0 + 9.0 + 16.0));
        assert!(h.app().open && h.app().chosen.is_none(), "a disabled row does nothing");
        h.click(Point::new(140.0, 80.0 + 6.0 + 32.0 + 9.0 + 32.0 + 16.0));
        assert_eq!(h.app().chosen, Some("delete"));
        assert!(!h.app().open);

        right_click(&mut h, Point::new(100.0, 80.0));
        h.click(Point::new(350.0, 250.0));
        assert!(!h.app().open, "a click elsewhere dismisses");
        assert_eq!(h.app().chosen, Some("delete"), "and chooses nothing new");

        // Near the corner the menu moves to stay inside the window.
        right_click(&mut h, Point::new(395.0, 295.0));
        h.render(1.0);
        h.click(Point::new(300.0, 190.0));
        assert_eq!(h.app().chosen, Some("open"));
    }
}

mod drag_and_drop {
    use super::*;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Shelf {
        clicks: u32,
        inner: Vec<PathBuf>,
        outer: Vec<PathBuf>,
    }

    #[derive(Clone)]
    enum Act {
        Click,
        Inner(Vec<PathBuf>),
        Outer(Vec<PathBuf>),
    }

    impl App for Shelf {
        type Message = Act;
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, m: Act) {
            match m {
                Act::Click => self.clicks += 1,
                Act::Inner(p) => self.inner = p,
                Act::Outer(p) => self.outer = p,
            }
        }
        fn view(&self) -> Element<Act> {
            // A draggable, droppable button at the top of a droppable window.
            let item = mouse_area(button("item").on_press(Act::Click).width(200.0).height(40.0)).drag_files(vec![PathBuf::from("/tmp/a.txt")]).on_drop(Act::Inner);
            mouse_area(container(column().push(item)).width(Length::Fill).height(Length::Fill)).on_drop(Act::Outer).into()
        }
    }

    #[test]
    fn a_drag_starts_after_some_movement_and_a_click_still_clicks() {
        let mut h = Harness::new(Shelf::default(), Size::new(400.0, 300.0)).unwrap();
        h.click(Point::new(50.0, 20.0));
        assert_eq!(h.app().clicks, 1);
        assert!(h.take_window_requests().is_empty(), "a click is not a drag");

        h.event(neo::Event::PointerPressed { pos: Point::new(50.0, 20.0), button: neo::PointerButton::Primary });
        h.event(neo::Event::PointerMoved { pos: Point::new(52.0, 21.0) });
        assert!(h.take_window_requests().is_empty(), "a wobble is not a drag");
        h.event(neo::Event::PointerMoved { pos: Point::new(70.0, 30.0) });
        assert_eq!(h.take_window_requests(), [neo::WindowRequest::DragFiles(vec![PathBuf::from("/tmp/a.txt")])]);
        // The system takes over; the shell tells widgets the pointer left.
        h.event(neo::Event::PointerLeft);
        assert_eq!(h.app().clicks, 1, "starting a drag does not click");
    }

    #[test]
    fn the_innermost_area_under_the_pointer_gets_the_drop() {
        let mut h = Harness::new(Shelf::default(), Size::new(400.0, 300.0)).unwrap();
        let files = vec![PathBuf::from("/tmp/x.png")];
        h.event(neo::Event::FilesDropped { pos: Point::new(50.0, 20.0), paths: files.clone() });
        assert_eq!((h.app().inner.clone(), h.app().outer.len()), (files.clone(), 0));
        h.event(neo::Event::FilesDropped { pos: Point::new(300.0, 250.0), paths: files.clone() });
        assert_eq!(h.app().outer, files);
    }
}

mod long_text {
    use super::*;

    struct Columns;

    impl App for Columns {
        type Message = ();
        fn window(&self) -> WindowSettings {
            WindowSettings { decorations: Decorations::System, ..Default::default() }
        }
        fn update(&mut self, _: ()) {}
        fn view(&self) -> Element<()> {
            // A long name beside a fixed column, as in a file list.
            let long = "A very long file name that could never fit in the space it has been given here.txt";
            container(row().spacing(0.0).width(Length::Fill).push(text(long).no_wrap().width(Length::Fill)).push(container(Space::new(0.0, 0.0)).width(150.0))).background(Background::Color(Color::WHITE)).width(Length::Fill).height(Length::Fill).into()
        }
    }

    #[test]
    fn one_line_text_is_cut_short_instead_of_running_over_its_neighbour() {
        let mut h = Harness::new(Columns, Size::new(300.0, 40.0)).unwrap();
        let px = h.render(1.0);
        // The right-hand 150 pixels belong to the other column: nothing but background.
        let stray = (0..40).flat_map(|y| (155..300).map(move |x| (x, y))).filter(|(x, y)| px[(y * 300 + x) * 4] < 200).count();
        assert_eq!(stray, 0, "text ran into the next column");
        // And the name is still there on the left.
        let ink = (0..40).flat_map(|y| (0..150).map(move |x| (x, y))).filter(|(x, y)| px[(y * 300 + x) * 4] < 120).count();
        assert!(ink > 50, "the shortened name is drawn");
    }
}
