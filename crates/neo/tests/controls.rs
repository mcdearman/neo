//! What each control does, what the layout rules are, and what the app hooks
//! promise. Everything goes through the public API, so these tests keep
//! passing unchanged however the crates underneath are divided.
//!
//! Needs a GPU adapter, like `interaction.rs`.

use std::time::Duration;

use neo::prelude::*;
use neo::testing::Harness;
use neo::{CursorIcon, Key, Modifiers, Point, Size};

fn system() -> WindowSettings {
    WindowSettings { decorations: Decorations::System, ..Default::default() }
}

/// One pixel of a frame that is `w` pixels wide.
fn pixel(px: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
    let i = (y * w + x) * 4;
    [px[i], px[i + 1], px[i + 2]]
}

/// Whether a rendered pixel is `c`, allowing for rounding.
fn is(got: [u8; 3], c: Color) -> bool {
    let want = c.to_rgba8();
    got.iter().zip(want.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 3)
}

const RED: Color = Color::hex(0xff0000);
const GREEN: Color = Color::hex(0x00ff00);
const BLUE: Color = Color::hex(0x0000ff);

/// A solid block that fills whatever space it is given.
fn block<M: 'static>(c: Color) -> Container<M> {
    container(Space::new(Length::Fill, Length::Fill)).width(Length::Fill).height(Length::Fill).background(Background::Color(c))
}

/// A solid block of a fixed size.
fn tile<M: 'static>(c: Color, w: f32, h: f32) -> Container<M> {
    container(Space::new(0.0, 0.0)).width(w).height(h).background(Background::Color(c))
}

mod controls {
    use super::*;

    #[derive(Default)]
    struct Panel {
        on: bool,
        checked: bool,
        choice: Option<usize>,
        level: f32,
        released: u32,
        presses: Vec<Modifiers>,
    }

    #[derive(Clone, Debug)]
    enum Msg {
        On(bool),
        Checked(bool),
        Choice(usize),
        Level(f32),
        Released,
        Go(Modifiers),
    }

    impl App for Panel {
        type Message = Msg;

        fn window(&self) -> WindowSettings {
            system()
        }

        fn update(&mut self, m: Msg) {
            match m {
                Msg::On(b) => self.on = b,
                Msg::Checked(b) => self.checked = b,
                Msg::Choice(i) => self.choice = Some(i),
                Msg::Level(l) => self.level = l,
                Msg::Released => self.released += 1,
                Msg::Go(m) => self.presses.push(m),
            }
        }

        // 20px padding; each control sits at the top left of a 300 by 40 row,
        // and rows are 50px apart. Row `i` starts at y = 20 + 50 * i.
        fn view(&self) -> Element<Msg> {
            let slot = |e: Element<Msg>| container(e).width(300.0).height(40.0);
            column()
                .padding(20.0)
                .spacing(10.0)
                .push(slot(toggle(self.on, Msg::On).into()))
                .push(slot(checkbox("Remember", self.checked, Msg::Checked).into()))
                .push(slot(segmented(["One", "Two", "Three"], self.choice, Msg::Choice).width(Length::Fill).into()))
                .push(slot(slider(0.0..=100.0, self.level, Msg::Level).step(10.0).on_release(Msg::Released).width(300.0).into()))
                .push(slot(button("Go").on_press_with(Msg::Go).width(120.0).height(40.0).into()))
                .push(slot(button("Off").width(120.0).height(40.0).into()))
                .into()
        }
    }

    fn row(i: usize) -> f32 {
        20.0 + 50.0 * i as f32
    }

    fn harness() -> Harness<Panel> {
        Harness::new(Panel::default(), Size::new(400.0, 340.0)).unwrap()
    }

    fn tab(h: &mut Harness<Panel>, times: usize) {
        for _ in 0..times {
            h.key(Key::Tab, Modifiers::default());
        }
    }

    #[test]
    fn a_toggle_flips_on_click_and_on_space() {
        let mut h = harness();
        h.click(Point::new(30.0, row(0) + 8.0));
        assert!(h.app().on);
        h.click(Point::new(30.0, row(0) + 8.0));
        assert!(!h.app().on);

        let mut h = harness();
        tab(&mut h, 1);
        h.key(Key::Space, Modifiers::default());
        assert!(h.app().on, "the toggle is the first stop for Tab");
    }

    #[test]
    fn a_checkbox_flips_when_its_box_or_label_is_clicked() {
        let mut h = harness();
        h.click(Point::new(28.0, row(1) + 8.0));
        assert!(h.app().checked, "the box");
        h.click(Point::new(70.0, row(1) + 8.0));
        assert!(!h.app().checked, "the label");
    }

    #[test]
    fn a_segmented_control_picks_the_segment_under_the_pointer() {
        let mut h = harness();
        // A filling control shares its 300px evenly: three segments from x = 20.
        h.click(Point::new(20.0 + 250.0, row(2) + 10.0));
        assert_eq!(h.app().choice, Some(2));
        h.click(Point::new(20.0 + 50.0, row(2) + 10.0));
        assert_eq!(h.app().choice, Some(0));
        h.click(Point::new(20.0 + 150.0, row(2) + 10.0));
        assert_eq!(h.app().choice, Some(1));
    }

    #[test]
    fn arrow_keys_move_a_segmented_control_and_stop_at_its_ends() {
        let mut h = harness();
        h.app_mut().choice = Some(1);
        tab(&mut h, 3);
        h.key(Key::Right, Modifiers::default());
        assert_eq!(h.app().choice, Some(2));
        h.key(Key::Right, Modifiers::default());
        assert_eq!(h.app().choice, Some(2), "no wrapping past the last segment");
        h.key(Key::Left, Modifiers::default());
        h.key(Key::Left, Modifiers::default());
        h.key(Key::Left, Modifiers::default());
        assert_eq!(h.app().choice, Some(0), "no wrapping past the first segment");
    }

    #[test]
    fn a_slider_follows_the_pointer_and_reports_the_release() {
        let mut h = harness();
        h.click(Point::new(20.0 + 150.0, row(3) + 10.0));
        assert_eq!(h.app().level, 50.0, "the middle of a 0 to 100 track, in steps of 10");
        assert_eq!(h.app().released, 1);
        h.click(Point::new(20.0 + 299.0, row(3) + 10.0));
        assert_eq!(h.app().level, 100.0);
        assert_eq!(h.app().released, 2);
    }

    #[test]
    fn keys_move_a_slider_by_steps_and_to_its_ends() {
        let mut h = harness();
        tab(&mut h, 4);
        let none = Modifiers::default();
        h.key(Key::Right, none);
        assert_eq!(h.app().level, 10.0);
        h.key(Key::Up, none);
        assert_eq!(h.app().level, 20.0);
        h.key(Key::Left, none);
        assert_eq!(h.app().level, 10.0);
        h.key(Key::End, none);
        assert_eq!(h.app().level, 100.0);
        h.key(Key::Right, none);
        assert_eq!(h.app().level, 100.0, "clamped at the top");
        h.key(Key::Home, none);
        assert_eq!(h.app().level, 0.0);
        h.key(Key::PageUp, none);
        assert_eq!(h.app().level, 10.0, "a page is a tenth of the range");
    }

    #[test]
    fn a_button_reports_the_modifier_keys_held() {
        let mut h = harness();
        h.set_modifiers(Modifiers { shift: true, ..Default::default() });
        h.click(Point::new(40.0, row(4) + 20.0));
        h.set_modifiers(Modifiers::default());
        h.click(Point::new(40.0, row(4) + 20.0));
        assert_eq!(h.app().presses.len(), 2);
        assert!(h.app().presses[0].shift);
        assert!(!h.app().presses[1].shift);

        // From the keyboard, the modifiers come with the key press.
        let mut h = harness();
        tab(&mut h, 5);
        h.key(Key::Enter, Modifiers { alt: true, ..Default::default() });
        assert_eq!(h.app().presses.len(), 1);
        assert!(h.app().presses[0].alt);
    }

    #[test]
    fn releasing_outside_a_button_cancels_the_press() {
        let mut h = harness();
        let inside = Point::new(40.0, row(4) + 20.0);
        let outside = Point::new(300.0, row(4) + 20.0);
        h.move_to(inside);
        h.event(neo::Event::PointerPressed { pos: inside, button: neo::PointerButton::Primary });
        h.move_to(outside);
        h.event(neo::Event::PointerReleased { pos: outside, button: neo::PointerButton::Primary });
        assert!(h.app().presses.is_empty());
    }

    #[test]
    fn a_button_without_a_message_is_disabled() {
        let mut h = harness();
        let off = Point::new(40.0, row(5) + 20.0);
        h.click(off);
        assert!(h.app().presses.is_empty());
        assert_ne!(h.cursor(), CursorIcon::Pointer, "a disabled button does not invite a click");
        h.move_to(Point::new(40.0, row(4) + 20.0));
        assert_eq!(h.cursor(), CursorIcon::Pointer);

        // Tab skips it: five stops, so the sixth press is back on the toggle.
        let mut h = harness();
        tab(&mut h, 6);
        h.key(Key::Space, Modifiers::default());
        assert!(h.app().on);
    }

    #[test]
    fn shift_tab_walks_backwards() {
        let mut h = harness();
        tab(&mut h, 2);
        h.key(Key::Tab, Modifiers { shift: true, ..Default::default() });
        h.key(Key::Space, Modifiers::default());
        assert!(h.app().on, "back on the toggle");
        assert!(!h.app().checked);
    }

    #[test]
    fn hovering_a_button_changes_how_it_looks() {
        let mut h = harness();
        let rest = h.render(1.0);
        h.move_to(Point::new(40.0, row(4) + 20.0));
        let hovered = h.render(1.0);
        let at = |px: &[u8]| pixel(px, 400, 30, row(4) as usize + 6);
        assert_ne!(at(&rest), at(&hovered));
        // And only the button: the toggle's row is untouched.
        let band = |px: &[u8]| px[..400 * 60 * 4].to_vec();
        assert_eq!(band(&rest), band(&hovered));
    }
}

mod text_field {
    use super::*;

    #[derive(Default)]
    struct Search {
        query: String,
        submitted: u32,
        cancelled: u32,
        shortcuts: u32,
    }

    #[derive(Clone, Debug)]
    enum Msg {
        Query(String),
        Submit,
        Cancel,
        Shortcut,
    }

    impl App for Search {
        type Message = Msg;

        fn window(&self) -> WindowSettings {
            system()
        }

        fn update(&mut self, m: Msg) {
            match m {
                Msg::Query(q) => self.query = q,
                Msg::Submit => self.submitted += 1,
                Msg::Cancel => self.cancelled += 1,
                Msg::Shortcut => self.shortcuts += 1,
            }
        }

        fn on_key(&self, k: &neo::KeyEvent) -> Option<Msg> {
            (k.pressed && k.key == Key::Character("k".into())).then_some(Msg::Shortcut)
        }

        fn view(&self) -> Element<Msg> {
            column()
                .padding(20.0)
                .push(text_input("Search", self.query.clone()).on_input(Msg::Query).on_submit(Msg::Submit).on_cancel(Msg::Cancel).autofocus(true).width(300.0))
                .into()
        }
    }

    fn harness() -> Harness<Search> {
        let mut h = Harness::new(Search::default(), Size::new(400.0, 100.0)).unwrap();
        // The field takes focus when it is first laid out.
        h.render(1.0);
        h
    }

    #[test]
    fn an_autofocused_field_takes_typing_straight_away() {
        let mut h = harness();
        h.type_text("neo");
        assert_eq!(h.app().query, "neo");
    }

    #[test]
    fn enter_submits_and_escape_cancels() {
        let mut h = harness();
        h.type_text("a");
        h.key(Key::Enter, Modifiers::default());
        assert_eq!((h.app().submitted, h.app().cancelled), (1, 0));
        h.key(Key::Escape, Modifiers::default());
        assert_eq!((h.app().submitted, h.app().cancelled), (1, 1));
        assert_eq!(h.app().query, "a", "neither key edits the text");
    }

    #[test]
    fn editing_keys_work_on_whole_characters() {
        let mut h = harness();
        h.type_text("héllo");
        let none = Modifiers::default();
        h.key(Key::Backspace, none);
        assert_eq!(h.app().query, "héll");
        h.key(Key::Home, none);
        h.key(Key::Right, none);
        h.key(Key::Delete, none);
        assert_eq!(h.app().query, "hll", "delete removes the two-byte é whole");
        h.key(Key::End, none);
        h.type_text("!");
        assert_eq!(h.app().query, "hll!");
    }

    #[test]
    fn the_caret_moves_to_the_end_when_the_app_replaces_the_text() {
        let mut h = harness();
        h.type_text("ab");
        h.key(Key::Home, Modifiers::default());
        h.app_mut().query = "chosen".into();
        h.render(1.0);
        h.type_text("!");
        assert_eq!(h.app().query, "chosen!");
    }

    #[test]
    fn keys_a_focused_field_uses_do_not_reach_app_shortcuts() {
        let mut h = harness();
        h.type_text("k");
        assert_eq!(h.app().query, "k");
        assert_eq!(h.app().shortcuts, 0);
        // Clicking empty space gives up focus, and then the key is the app's.
        h.click(Point::new(380.0, 90.0));
        h.type_text("k");
        assert_eq!(h.app().query, "k");
        assert_eq!(h.app().shortcuts, 1);
    }

    #[test]
    fn the_pointer_becomes_a_text_cursor_over_a_field() {
        let mut h = harness();
        h.move_to(Point::new(100.0, 35.0));
        assert_eq!(h.cursor(), CursorIcon::Text);
        h.move_to(Point::new(380.0, 90.0));
        assert_eq!(h.cursor(), CursorIcon::Default);
    }
}

mod app_hooks {
    use super::*;

    #[derive(Default)]
    struct Clock {
        ticks: u32,
        running: bool,
        hidden: bool,
        done: bool,
        focus: Vec<bool>,
        keeps_open: bool,
    }

    #[derive(Clone, Debug)]
    enum Msg {
        Tick,
        Hide,
        Focus(bool),
    }

    impl App for Clock {
        type Message = Msg;

        fn title(&self) -> String {
            format!("Clock {}", self.ticks)
        }

        fn window(&self) -> WindowSettings {
            system()
        }

        fn update(&mut self, m: Msg) {
            match m {
                Msg::Tick => self.ticks += 1,
                Msg::Hide => self.hidden = true,
                Msg::Focus(f) => self.focus.push(f),
            }
        }

        fn subscriptions(&self) -> Vec<Subscription<Msg>> {
            if self.running { vec![Subscription::every(Duration::from_millis(100), Msg::Tick)] } else { vec![] }
        }

        fn window_state(&self) -> WindowState {
            WindowState { visible: !self.hidden, ..Default::default() }
        }

        fn on_close(&self) -> Option<Msg> {
            self.keeps_open.then_some(Msg::Hide)
        }

        fn on_window_focus(&self, focused: bool) -> Option<Msg> {
            Some(Msg::Focus(focused))
        }

        fn should_exit(&self) -> bool {
            self.done
        }

        fn view(&self) -> Element<Msg> {
            text(self.ticks.to_string()).into()
        }
    }

    fn harness(app: Clock) -> Harness<Clock> {
        Harness::new(app, Size::new(200.0, 100.0)).unwrap()
    }

    #[test]
    fn a_subscription_sends_its_message_once_per_period() {
        let mut h = harness(Clock { running: true, ..Default::default() });
        h.advance(Duration::from_millis(50));
        assert_eq!(h.app().ticks, 0, "the first period has not passed");
        h.advance(Duration::from_millis(100));
        assert_eq!(h.app().ticks, 1);
        h.advance(Duration::from_millis(100));
        assert_eq!(h.app().ticks, 2);
        assert_eq!(h.title(), "Clock 2", "the title follows the state");

        // Dropping the subscription stops the timer.
        h.app_mut().running = false;
        h.advance(Duration::from_millis(500));
        assert_eq!(h.app().ticks, 2);
    }

    #[test]
    fn a_long_stall_does_not_fire_a_burst_of_ticks() {
        let mut h = harness(Clock { running: true, ..Default::default() });
        h.advance(Duration::from_millis(10));
        h.advance(Duration::from_secs(5));
        assert_eq!(h.app().ticks, 1);
        h.advance(Duration::from_millis(100));
        assert_eq!(h.app().ticks, 2);
    }

    #[test]
    fn closing_ends_the_app_unless_it_handles_the_request() {
        let mut h = harness(Clock::default());
        assert!(h.close(), "nothing handles it, so the window closes");

        let mut h = harness(Clock { keeps_open: true, ..Default::default() });
        assert!(h.window_state().visible);
        assert!(!h.close(), "the app took the request");
        assert!(!h.window_state().visible, "and hid its window instead");
    }

    #[test]
    fn the_app_hears_about_window_focus_and_can_ask_to_exit() {
        let mut h = harness(Clock::default());
        h.set_window_focused(false);
        h.set_window_focused(true);
        assert_eq!(h.app().focus, [false, true]);
        assert!(!h.should_exit());
        h.app_mut().done = true;
        assert!(h.should_exit());
    }

    struct Themed(Option<Accent>);

    impl App for Themed {
        type Message = ();

        fn window(&self) -> WindowSettings {
            system()
        }

        fn update(&mut self, _: ()) {}

        fn theme(&self, system: Scheme) -> Theme {
            Theme { scheme: system, accent: self.0.unwrap_or_default(), ..Theme::default() }
        }

        fn view(&self) -> Element<()> {
            column().padding(20.0).push(button("Save").kind(ButtonKind::Accent).on_press(()).width(160.0).height(60.0)).into()
        }
    }

    #[test]
    fn the_window_follows_the_system_scheme() {
        let mut h = Harness::new(Themed(None), Size::new(200.0, 100.0)).unwrap();
        assert_eq!(h.theme().scheme, Scheme::Light);
        let light = pixel(&h.render(1.0), 200, 190, 90);
        assert!(is(light, h.theme().palette().bg));

        h.set_system_scheme(Scheme::Dark);
        assert_eq!(h.theme().scheme, Scheme::Dark);
        let dark = pixel(&h.render(1.0), 200, 190, 90);
        assert!(is(dark, h.theme().palette().bg));
        assert_ne!(light, dark);
    }

    #[test]
    fn controls_take_their_colours_from_the_apps_theme() {
        for scheme in [Scheme::Light, Scheme::Dark] {
            for accent in [Accent::Royal, Accent::Coral] {
                let mut h = Harness::new(Themed(Some(accent)), Size::new(200.0, 100.0)).unwrap();
                h.set_system_scheme(scheme);
                let theme = h.theme();
                assert_eq!(theme.accent, accent);
                // Inside the accent button, left of its centred label.
                let got = pixel(&h.render(1.0), 200, 40, 50);
                let want = theme.paint(Surface::Accent).fill;
                assert!(is(got, want), "{scheme:?} {accent:?}: button is {got:?}, theme says {:?}", want.to_rgba8());
            }
        }
    }
}

mod surfaces {
    use super::*;

    struct Swatches(Scheme);

    impl App for Swatches {
        type Message = ();

        fn window(&self) -> WindowSettings {
            system()
        }

        fn update(&mut self, _: ()) {}

        fn theme(&self, _: Scheme) -> Theme {
            Theme { scheme: self.0, ..Theme::default() }
        }

        // 20px padding, then 60px squares 20px apart.
        fn view(&self) -> Element<()> {
            let swatch = |s: Surface| container(Space::new(0.0, 0.0)).width(60.0).height(60.0).surface(s);
            row()
                .padding(20.0)
                .spacing(20.0)
                .push(swatch(Surface::Card))
                .push(swatch(Surface::Raised))
                .push(swatch(Surface::Well))
                .push(swatch(Surface::Accent))
                .push(button("").on_press(()).width(60.0).height(60.0))
                .push(progress_bar(1.0).width(60.0).height(20.0))
                .into()
        }
    }

    #[test]
    fn surfaces_and_controls_are_filled_as_the_theme_says() {
        for scheme in [Scheme::Light, Scheme::Dark] {
            let mut h = Harness::new(Swatches(scheme), Size::new(520.0, 100.0)).unwrap();
            let theme = h.theme();
            let px = h.render(1.0);
            let centre = |i: usize| pixel(&px, 520, 50 + 80 * i, 50);
            for (i, s) in [Surface::Card, Surface::Raised, Surface::Well, Surface::Accent].into_iter().enumerate() {
                let want = theme.paint(s).fill;
                assert!(is(centre(i), want), "{scheme:?} {s:?}: got {:?}, theme says {:?}", centre(i), want.to_rgba8());
            }
            assert!(is(centre(4), theme.paint(Surface::Raised).fill), "{scheme:?}: a button at rest is a raised surface");
            let bar = pixel(&px, 520, 50 + 80 * 5, 30);
            assert!(is(bar, theme.palette().accent), "{scheme:?}: a full progress bar is the accent, got {bar:?}");
        }
    }
}

mod layout {
    use super::*;

    /// Renders a view in a 200 by 200 window and returns its pixels.
    fn draw(view: fn() -> Element<()>) -> Vec<u8> {
        draw_at(view, Size::new(200.0, 200.0), 1.0)
    }

    fn draw_at(view: fn() -> Element<()>, size: Size, scale: f32) -> Vec<u8> {
        struct Fixed(fn() -> Element<()>);
        impl App for Fixed {
            type Message = ();
            fn window(&self) -> WindowSettings {
                system()
            }
            fn update(&mut self, _: ()) {}
            fn view(&self) -> Element<()> {
                (self.0)()
            }
        }
        Harness::new(Fixed(view), size).unwrap().render(scale)
    }

    fn at(px: &[u8], x: usize, y: usize) -> [u8; 3] {
        pixel(px, 200, x, y)
    }

    const R: [u8; 3] = [255, 0, 0];
    const G: [u8; 3] = [0, 255, 0];
    const B: [u8; 3] = [0, 0, 255];

    #[test]
    fn four_padding_values_run_clockwise_from_the_top() {
        // Top 10, right 20, bottom 30, left 40.
        let px = draw(|| container(block(GREEN)).padding([10.0, 20.0, 30.0, 40.0]).width(Length::Fill).height(Length::Fill).background(Background::Color(RED)).into());
        assert_eq!((at(&px, 100, 9), at(&px, 100, 10)), (R, G), "top");
        assert_eq!((at(&px, 179, 100), at(&px, 180, 100)), (G, R), "right");
        assert_eq!((at(&px, 100, 169), at(&px, 100, 170)), (G, R), "bottom");
        assert_eq!((at(&px, 39, 100), at(&px, 40, 100)), (R, G), "left");
    }

    #[test]
    fn two_padding_values_are_horizontal_then_vertical() {
        let px = draw(|| container(block(GREEN)).padding([40.0, 10.0]).width(Length::Fill).height(Length::Fill).background(Background::Color(RED)).into());
        assert_eq!((at(&px, 39, 100), at(&px, 40, 100)), (R, G), "left");
        assert_eq!((at(&px, 159, 100), at(&px, 160, 100)), (G, R), "right");
        assert_eq!((at(&px, 100, 9), at(&px, 100, 10)), (R, G), "top");
        assert_eq!((at(&px, 100, 189), at(&px, 100, 190)), (G, R), "bottom");
    }

    #[test]
    fn one_padding_value_applies_to_every_side() {
        let px = draw(|| container(block(GREEN)).padding(25.0).width(Length::Fill).height(Length::Fill).background(Background::Color(RED)).into());
        for (x, y) in [(24, 100), (175, 100), (100, 24), (100, 175)] {
            assert_eq!(at(&px, x, y), R);
        }
        for (x, y) in [(25, 100), (174, 100), (100, 25), (100, 174)] {
            assert_eq!(at(&px, x, y), G);
        }
    }

    #[test]
    fn a_container_places_its_child_by_alignment() {
        let px = draw(|| container(tile(BLUE, 20.0, 20.0)).width(Length::Fill).height(Length::Fill).into());
        assert_eq!(at(&px, 10, 10), B, "top left by default");
        assert_ne!(at(&px, 30, 30), B);

        let px = draw(|| container(tile(BLUE, 20.0, 20.0)).width(Length::Fill).height(Length::Fill).center().into());
        assert_eq!((at(&px, 91, 91), at(&px, 108, 108)), (B, B));
        assert_ne!(at(&px, 88, 100), B);
        assert_ne!(at(&px, 111, 100), B);

        let px = draw(|| container(tile(BLUE, 20.0, 20.0)).width(Length::Fill).height(Length::Fill).align_x(Align::End).align_y(Align::End).into());
        assert_eq!((at(&px, 181, 181), at(&px, 199, 199)), (B, B));
        assert_ne!(at(&px, 178, 190), B);
    }

    #[test]
    fn max_width_caps_a_filling_container() {
        let px = draw(|| container(block(BLUE)).width(Length::Fill).max_width(120.0).height(Length::Fill).into());
        assert_eq!(at(&px, 119, 100), B);
        assert_ne!(at(&px, 121, 100), B);
    }

    #[test]
    fn a_column_stacks_children_with_spacing_between_them() {
        let px = draw(|| column().spacing(10.0).push(tile(RED, 50.0, 20.0)).push(tile(GREEN, 50.0, 20.0)).push(tile(BLUE, 50.0, 20.0)).into());
        assert_eq!((at(&px, 10, 0), at(&px, 10, 19)), (R, R));
        assert_ne!(at(&px, 10, 25), R);
        assert_eq!((at(&px, 10, 30), at(&px, 10, 49)), (G, G));
        assert_eq!((at(&px, 10, 60), at(&px, 10, 79)), (B, B));
        assert_ne!(at(&px, 10, 81), B);
    }

    #[test]
    fn justify_spreads_or_centres_a_rows_children() {

        let px = draw(|| row().width(Length::Fill).justify(Justify::SpaceBetween).push(tile(RED, 20.0, 20.0)).push(tile(GREEN, 20.0, 20.0)).push(tile(BLUE, 20.0, 20.0)).into());
        assert_eq!((at(&px, 10, 10), at(&px, 100, 10), at(&px, 190, 10)), (R, G, B), "first, middle, last");

        let px = draw(|| row().width(Length::Fill).justify(Justify::Center).push(tile(RED, 20.0, 20.0)).push(tile(GREEN, 20.0, 20.0)).push(tile(BLUE, 20.0, 20.0)).into());
        assert_eq!((at(&px, 80, 10), at(&px, 100, 10), at(&px, 120, 10)), (R, G, B));
        assert_ne!(at(&px, 68, 10), R);

        let px = draw(|| row().width(Length::Fill).justify(Justify::End).push(tile(RED, 20.0, 20.0)).push(tile(GREEN, 20.0, 20.0)).push(tile(BLUE, 20.0, 20.0)).into());
        assert_eq!((at(&px, 150, 10), at(&px, 170, 10), at(&px, 190, 10)), (R, G, B));
    }

    #[test]
    fn a_filling_space_pushes_what_follows_to_the_far_edge() {
        let px = draw(|| row().width(Length::Fill).push(tile(RED, 20.0, 20.0)).push(Space::fill_x()).push(tile(BLUE, 20.0, 20.0)).into());
        assert_eq!((at(&px, 10, 10), at(&px, 190, 10)), (R, B));
        assert_ne!(at(&px, 100, 10), R);
    }

    #[test]
    fn a_stack_draws_later_children_over_earlier_ones() {
        let px = draw(|| stack().push(block(RED)).push(tile(BLUE, 50.0, 50.0)).into());
        assert_eq!(at(&px, 25, 25), B);
        assert_eq!(at(&px, 100, 100), R);
    }

    /// The first row, scanning down column `x`, that is `c`.
    fn first_row(px: &[u8], x: usize, c: [u8; 3]) -> usize {
        (0..200).find(|y| at(px, x, *y) == c).expect("the marker is drawn")
    }

    #[test]
    fn text_wraps_to_its_width_and_one_line_text_does_not() {
        const LONG: &str = "The quick brown fox jumps over the lazy dog and keeps on running";
        let short = draw(|| column().push(text("Fox").width(100.0)).push(tile(BLUE, 100.0, 10.0)).into());
        let wrapped = draw(|| column().push(text(LONG).width(100.0)).push(tile(BLUE, 100.0, 10.0)).into());
        let single = draw(|| column().push(text(LONG).width(100.0).no_wrap()).push(tile(BLUE, 100.0, 10.0)).into());
        let line = first_row(&short, 50, B);
        assert!(first_row(&wrapped, 50, B) >= line * 3, "the long text takes several lines");
        assert_eq!(first_row(&single, 50, B), line, "one-line text stays one line high");
    }

    #[test]
    fn a_larger_window_gives_filling_children_more_room() {
        struct Fill;
        impl App for Fill {
            type Message = ();
            fn window(&self) -> WindowSettings {
                system()
            }
            fn update(&mut self, _: ()) {}
            fn view(&self) -> Element<()> {
                block(BLUE).into()
            }
        }
        let mut h = Harness::new(Fill, Size::new(100.0, 50.0)).unwrap();
        assert_eq!(pixel(&h.render(1.0), 100, 99, 49), B);
        h.resize(Size::new(300.0, 80.0));
        let px = h.render(1.0);
        assert_eq!(px.len(), 300 * 80 * 4);
        assert_eq!(pixel(&px, 300, 299, 79), B);
    }

    #[test]
    fn the_scale_factor_multiplies_pixels_not_layout() {
        let view: fn() -> Element<()> = || container(tile(BLUE, 50.0, 50.0)).width(Length::Fill).height(Length::Fill).background(Background::Color(RED)).into();
        let px = draw_at(view, Size::new(100.0, 100.0), 2.0);
        assert_eq!(px.len(), 200 * 200 * 4);
        assert_eq!(at(&px, 98, 98), B, "a 50px tile covers 100 physical pixels");
        assert_eq!(at(&px, 101, 101), R);
    }
}
