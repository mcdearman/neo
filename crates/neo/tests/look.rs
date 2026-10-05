//! How the toolkit looks. Each scene is rendered and compared with a picture
//! kept in `tests/golden/<platform>/`, so a change to the theme, the
//! renderer or a control's painting shows up as a failing test with the new
//! picture saved beside it for inspection.
//!
//! Pictures differ a little between graphics drivers, so each platform has
//! its own set, and a platform with no pictures yet skips the comparison.
//! After an intended change, look at the new pictures and accept them with
//! `NEO_BLESS=1 cargo test -p neo --test look`.

use std::path::{Path, PathBuf};

use neo::prelude::*;
use neo::testing::Harness;
use neo::{Image, Key, Modifiers, Point, Size};

/// Channels may differ by this much before a pixel counts as changed, which
/// absorbs rounding.
const CHANNEL_SLACK: i32 = 4;
/// Share of pixels that may change. A one-pixel border around a single
/// button is about ten times this.
const CHANGED_SHARE: f64 = 0.0001;

// The gallery only draws; nothing reads what its controls would send.
#[allow(dead_code)]
#[derive(Clone, Debug)]
enum Msg {
    Nothing,
    Flag(bool),
    Pick(usize),
    Level(f32),
    Text(String),
}

/// One of everything, in fixed states.
struct Gallery {
    theme: Theme,
    picture: Image,
    doc: Document,
    menu: bool,
}

impl Gallery {
    fn new(theme: Theme) -> Self {
        // A 4 by 4 checkerboard with a see-through corner.
        let mut rgba = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                let on = (x + y) % 2 == 0;
                let a = if x == 0 && y == 0 { 0 } else { 255 };
                rgba.extend_from_slice(&[if on { 230 } else { 30 }, 90, if on { 40 } else { 220 }, a]);
            }
        }
        let doc = Document::new("fn main() {\n    let neo = \"toolkit\"; // comment\n    println!(\"{neo}\");\n}\n");
        Self { theme, picture: Image::new(4, 4, rgba), doc, menu: false }
    }
}

impl App for Gallery {
    type Message = Msg;

    fn title(&self) -> String {
        "Gallery".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings::default()
    }

    fn theme(&self, _: Scheme) -> Theme {
        self.theme
    }

    fn update(&mut self, _: Msg) {}

    fn view(&self) -> Element<Msg> {
        let words = column()
            .spacing(4.0)
            .push(text("Display").role(TextRole::Display))
            .push(text("Heading").role(TextRole::Heading))
            .push(text("Title").role(TextRole::Title))
            .push(text("Strong").role(TextRole::Strong))
            .push(text("Body text in the default role"))
            .push(text("Caption").role(TextRole::Caption).tone(Tone::Muted))
            .push(text("Label").role(TextRole::Label).tone(Tone::Faint))
            .push(text("monospace 0123").mono())
            .push(
                row()
                    .spacing(8.0)
                    .push(text("accent").tone(Tone::Accent))
                    .push(text("good").tone(Tone::Good))
                    .push(text("warn").tone(Tone::Warn))
                    .push(text("bad").tone(Tone::Bad)),
            )
            .push(text("One line that is far too long for the space it has been given").width(150.0).no_wrap());

        let buttons = column()
            .spacing(8.0)
            .push(
                row()
                    .spacing(8.0)
                    .push(button("Raised").on_press(Msg::Nothing))
                    .push(button("Accent").kind(ButtonKind::Accent).on_press(Msg::Nothing))
                    .push(button("Ghost").kind(ButtonKind::Ghost).on_press(Msg::Nothing)),
            )
            .push(
                row()
                    .spacing(8.0)
                    .push(button("Selected").selected(true).on_press(Msg::Nothing))
                    .push(button("Disabled"))
                    .push(icon_button(icons::ACTIVITY, 32.0).on_press(Msg::Nothing)),
            )
            .push(row().spacing(12.0).push(toggle(true, Msg::Flag)).push(toggle(false, Msg::Flag)).push(checkbox("On", true, Msg::Flag)).push(checkbox("Off", false, Msg::Flag)))
            .push(segmented(["Day", "Week", "Month"], Some(1), Msg::Pick))
            .push(slider(0.0..=1.0, 0.35, Msg::Level).width(260.0))
            .push(text_input("Placeholder", "").on_input(Msg::Text).width(260.0))
            .push(text_input("", "Typed text").on_input(Msg::Text).width(260.0))
            .push(text_input("", "secret").secure(true).on_input(Msg::Text).width(260.0));

        let data = row()
            .spacing(16.0)
            .push(gauge(0.62, "62%"))
            .push(
                column()
                    .spacing(8.0)
                    .width(180.0)
                    .push(progress_bar(0.4))
                    .push(progress_bar(0.9).tone(Tone::Warn))
                    .push(sparkline(vec![1.0, 4.0, 2.0, 6.0, 3.0, 5.0], 0.0, 6.0).height(40.0)),
            )
            .push(picture(&self.picture).width(64.0).height(64.0))
            .push(
                row()
                    .spacing(6.0)
                    .push(icon(icons::ALARM_CLOCK).size(20.0))
                    .push(icon(icons::ACTIVITY).size(20.0).tone(Tone::Accent))
                    .push(icon(icons::AIRPLAY).size(20.0).tone(Tone::Muted)),
            );

        let surfaces = row()
            .spacing(10.0)
            .push(container(text("Card")).padding(12.0).surface(Surface::Card))
            .push(container(text("Raised")).padding(12.0).surface(Surface::Raised))
            .push(container(text("Well")).padding(12.0).surface(Surface::Well))
            .push(container(text("Inset")).padding(12.0).surface(Surface::Inset))
            .push(container(text("Accent")).padding(12.0).surface(Surface::Accent))
            .push(container(text("Glass")).padding(12.0).background(Background::Glass));

        let page = column()
            .padding(16.0)
            .spacing(14.0)
            .push(row().spacing(24.0).push(container(words).width(230.0)).push(buttons))
            .push(Divider::horizontal())
            .push(data)
            .push(surfaces)
            .push(container(text_editor(&self.doc).language(Language::Rust).height(90.0)).width(Length::Fill));

        if self.menu {
            let items = vec![
                MenuItem::new("Open", Msg::Nothing).icon(icons::ACTIVITY),
                MenuItem::new("Rename", Msg::Nothing),
                MenuItem::disabled("Paste"),
                MenuItem::separator(),
                MenuItem::new("Move to Trash", Msg::Nothing).danger(),
            ];
            stack().push(page).push(popup_menu(Point::new(180.0, 120.0), items, Msg::Nothing)).into()
        } else {
            page.into()
        }
    }
}

const SIZE: Size = Size::new(600.0, 760.0);

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(std::env::consts::OS)
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let mut reader = png::Decoder::new(file).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Compares what `h` shows with the picture called `name`.
fn check<A: App>(h: &mut Harness<A>, name: &str, size: Size) {
    let dir = golden_dir();
    let golden = dir.join(format!("{name}.png"));
    if std::env::var_os("NEO_BLESS").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        h.save_png(&golden, 1.0).unwrap();
        return;
    }
    if !golden.exists() {
        eprintln!("look: no picture of `{name}` for this platform, so nothing to compare. Make them with NEO_BLESS=1.");
        return;
    }
    let got = h.render(1.0);
    let (w, h_px, want) = read_png(&golden);
    assert_eq!((w, h_px), (size.w as u32, size.h as u32), "{name}: the picture is a different size");
    let changed = got.chunks_exact(4).zip(want.chunks_exact(4)).filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| (*x as i32 - *y as i32).abs() > CHANNEL_SLACK)).count();
    let allowed = ((w * h_px) as f64 * CHANGED_SHARE) as usize;
    if changed > allowed {
        let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("look");
        std::fs::create_dir_all(&out).unwrap();
        let actual = out.join(format!("{name}.png"));
        h.save_png(&actual, 1.0).unwrap();
        panic!("{name}: {changed} pixels differ from {} (up to {allowed} allowed). What was drawn is in {}", golden.display(), actual.display());
    }
}

fn gallery(theme: Theme) -> Harness<Gallery> {
    Harness::new(Gallery::new(theme), SIZE).expect("a GPU adapter is required for these tests")
}

#[test]
fn light() {
    check(&mut gallery(Theme::default()), "gallery-light", SIZE);
}

#[test]
fn dark() {
    check(&mut gallery(Theme { scheme: Scheme::Dark, ..Theme::default() }), "gallery-dark", SIZE);
}

#[test]
fn another_accent_and_radius() {
    check(&mut gallery(Theme { accent: Accent::Coral, radius: 8.0, text_scale: 1.1, ..Theme::default() }), "gallery-coral", SIZE);
}

#[test]
fn glass() {
    let theme = Theme { scheme: Scheme::Dark, glass: Glass { enabled: true, ..Glass::default() }, ..Theme::default() };
    check(&mut gallery(theme), "gallery-glass", SIZE);
}

#[test]
fn hover_and_keyboard_focus() {
    let mut h = gallery(Theme::default());
    // Tab to the first button for a focus ring, then rest the pointer on
    // the accent button.
    h.key(Key::Tab, Modifiers::default());
    h.move_to(Point::new(390.0, 75.0));
    check(&mut h, "gallery-active", SIZE);
}

#[test]
fn an_open_menu() {
    for (scheme, name) in [(Scheme::Light, "menu-light"), (Scheme::Dark, "menu-dark")] {
        let mut h = gallery(Theme { scheme, ..Theme::default() });
        h.app_mut().menu = true;
        h.move_to(Point::new(230.0, 170.0));
        check(&mut h, name, SIZE);
    }
}
