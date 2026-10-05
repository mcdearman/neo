//! Neo Photos: view pictures.
//!
//!     cargo run -p neo-photos -- picture.jpg     # or a folder of pictures
//!     cargo run -p neo-photos -- --snapshot target/snapshots
//!
//! Scroll or pinch to zoom, drag to move around, double-click to switch
//! between fitting the window and actual size. Left and Right walk through
//! the other pictures in the folder. `+` `-` zoom, `0` fits, `1` is actual
//! size, `R` turns the picture, `P` pauses an animation, and Delete moves it
//! to the Trash.
//!
//! Reads PNG, JPEG, GIF, WebP, BMP, TIFF and ICO everywhere, playing
//! animated GIFs and WebPs,
//! and on macOS anything the system can, including HEIC.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod decode;

use std::path::{Path, PathBuf};
use std::time::Instant;

use neo::prelude::*;
use neo::{Cx, CursorIcon, DrawCx, Event, EventCx, Image, Key, KeyEvent, Limits, Point, PointerButton, Proxy, Rect, Size, Status, Widget};
use neo_desktop::fs::{has_extension, human_size, move_to_trash, natural_cmp, reveal, IMAGE_EXTENSIONS};
use neo_desktop::ui::notice;
use neo_desktop::Desktop;

use decode::Decoded;

/// The most a picture can be magnified.
const MAX_SCALE: f32 = 16.0;
/// How much the zoom buttons and keys change the scale.
const STEP: f32 = 1.25;

/// How the picture sits in the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
struct View {
    /// Logical pixels per picture pixel. `None` fits the picture to the viewer.
    scale: Option<f32>,
    /// How far the picture's centre is from the viewer's.
    pan: Point,
    /// Quarter turns clockwise.
    turns: u8,
}

impl View {
    const FIT: View = View { scale: None, pan: Point::ZERO, turns: 0 };

    /// The picture's size as shown, after turning.
    fn shown(&self, image: Size) -> Size {
        if self.turns % 2 == 1 { Size::new(image.h, image.w) } else { image }
    }

    /// The scale that fits the whole picture, never magnifying a small one.
    fn fit_scale(&self, image: Size, viewer: Size) -> f32 {
        let s = self.shown(image);
        if s.w <= 0.0 || s.h <= 0.0 {
            return 1.0;
        }
        (viewer.w / s.w).min(viewer.h / s.h).min(1.0)
    }

    fn scale_in(&self, image: Size, viewer: Size) -> f32 {
        self.scale.unwrap_or_else(|| self.fit_scale(image, viewer))
    }

    /// Where the picture is drawn, in the viewer's own coordinates.
    fn rect(&self, image: Size, viewer: Size) -> Rect {
        let scale = self.scale_in(image, viewer);
        let s = self.shown(image);
        let (w, h) = (s.w * scale, s.h * scale);
        Rect::new((viewer.w - w) * 0.5 + self.pan.x, (viewer.h - h) * 0.5 + self.pan.y, w, h)
    }

    /// Keeps the picture from being dragged out of sight: centred on an
    /// axis where it fits, edge to edge at most where it does not.
    fn clamped(mut self, image: Size, viewer: Size) -> Self {
        let scale = self.scale_in(image, viewer);
        let s = self.shown(image);
        let limit = |shown: f32, room: f32| ((shown * scale - room) * 0.5).max(0.0);
        let (lx, ly) = (limit(s.w, viewer.w), limit(s.h, viewer.h));
        self.pan = Point::new(self.pan.x.clamp(-lx, lx), self.pan.y.clamp(-ly, ly));
        self
    }

    /// Zooms by `factor`, keeping the point `about` (measured from the
    /// viewer's centre) over the same part of the picture.
    fn zoomed(self, factor: f32, about: Point, image: Size, viewer: Size) -> Self {
        let fit = self.fit_scale(image, viewer);
        let old = self.scale_in(image, viewer);
        let new = (old * factor).clamp(fit.min(0.05), MAX_SCALE);
        // Zooming out to where it fits goes back to following the window.
        if new <= fit * 1.001 {
            return View { scale: None, pan: Point::ZERO, ..self };
        }
        let k = new / old;
        let pan = Point::new(about.x - (about.x - self.pan.x) * k, about.y - (about.y - self.pan.y) * k);
        View { scale: Some(new), pan, ..self }.clamped(image, viewer)
    }
}

struct Photos {
    desktop: Desktop,
    /// The pictures in the folder, in the order Files shows them.
    files: Vec<PathBuf>,
    index: usize,
    current: Option<Decoded>,
    /// The path being read, so a slow answer for another picture is dropped.
    loading: Option<PathBuf>,
    error: Option<String>,
    view: View,
    /// Which frame of an animation is showing, and whether it is playing.
    frame: usize,
    playing: bool,
    viewer: Size,
    status: Option<(Tone, String)>,
    proxy: Option<Proxy<Msg>>,
}

#[derive(Clone, Debug)]
enum Msg {
    Loaded(PathBuf, Result<Decoded, String>),
    Step(isize),
    View(View),
    ViewerSize(Size),
    Zoom(f32),
    Fit,
    Actual,
    Turn(u8),
    Reveal,
    Trash,
    /// Time for the animation's next frame.
    Advance,
    PlayPause,
    Poll,
}

/// The pictures beside `path`, and where `path` is among them. A folder
/// gives its own pictures.
fn siblings(path: &Path) -> (Vec<PathBuf>, usize) {
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file() && has_extension(p, IMAGE_EXTENSIONS)).collect();
    // A file opened by name is shown even if its extension is unusual.
    if path.is_file() && !files.iter().any(|f| f == path) {
        files.push(path.to_path_buf());
    }
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    files.sort_by(|a, b| natural_cmp(&name(a), &name(b)));
    let index = files.iter().position(|f| f == path).unwrap_or(0);
    (files, index)
}

impl Photos {
    fn new(path: Option<PathBuf>) -> Self {
        let (files, index) = path.as_deref().map(siblings).unwrap_or_default();
        Self { desktop: Desktop::load(), files, index, current: None, loading: None, error: None, view: View::FIT, frame: 0, playing: true, viewer: Size::new(800.0, 500.0), status: None, proxy: None }
    }

    fn path(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    fn image_size(&self) -> Size {
        self.current.as_ref().map_or(Size::ZERO, |d| Size::new(d.image.width() as f32, d.image.height() as f32))
    }

    /// Starts reading the current picture. The one on screen stays until
    /// the new one is ready.
    fn load(&mut self) {
        let Some(path) = self.path().cloned() else {
            self.current = None;
            self.loading = None;
            return;
        };
        self.loading = Some(path.clone());
        self.error = None;
        match self.proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let result = decode::decode(&path);
                    proxy.send(Msg::Loaded(path, result));
                });
            }
            None => {
                let result = decode::decode(&path);
                self.update(Msg::Loaded(path, result));
            }
        }
    }
}

impl App for Photos {
    type Message = Msg;

    fn title(&self) -> String {
        match self.path().and_then(|p| p.file_name()) {
            Some(name) => format!("{} — Photos", name.to_string_lossy()),
            None => "Photos".into(),
        }
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1040.0, 720.0), min_size: Some(Size::new(480.0, 360.0)), app_id: Some("org.neo.Photos".into()), ..Default::default() }
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy);
        self.load();
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        // Each frame says how long it stays up; the timer follows it.
        if self.playing
            && let Some(frame) = self.current.as_ref().and_then(|d| d.frames.get(self.frame))
        {
            subs.push(Subscription::every(frame.delay, Msg::Advance));
        }
        subs
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match &k.key {
            Key::Left | Key::PageUp => Some(Msg::Step(-1)),
            Key::Right | Key::PageDown | Key::Space => Some(Msg::Step(1)),
            Key::Delete => Some(Msg::Trash),
            Key::Backspace if k.modifiers.command() => Some(Msg::Trash),
            Key::Character(c) => match c.as_str() {
                "+" | "=" => Some(Msg::Zoom(STEP)),
                "-" | "_" => Some(Msg::Zoom(1.0 / STEP)),
                "0" => Some(Msg::Fit),
                "1" => Some(Msg::Actual),
                "r" => Some(Msg::Turn(if k.modifiers.shift { 3 } else { 1 })),
                "p" => Some(Msg::PlayPause),
                _ => None,
            },
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Advance => {
                if let Some(d) = &self.current
                    && !d.frames.is_empty()
                {
                    self.frame = (self.frame + 1) % d.frames.len();
                }
            }
            Msg::PlayPause => self.playing = !self.playing,
            Msg::Loaded(path, result) => {
                self.frame = 0;
                self.playing = true;
                if self.loading.as_ref() != Some(&path) {
                    return;
                }
                self.loading = None;
                self.view = View::FIT;
                match result {
                    Ok(decoded) => self.current = Some(decoded),
                    Err(e) => {
                        self.current = None;
                        self.error = Some(e);
                    }
                }
            }
            Msg::Step(by) => {
                if self.files.len() > 1 {
                    let n = self.files.len() as isize;
                    self.index = (self.index as isize + by).rem_euclid(n) as usize;
                    self.status = None;
                    self.load();
                }
            }
            Msg::View(v) => self.view = v.clamped(self.image_size(), self.viewer),
            Msg::ViewerSize(s) => {
                self.viewer = s;
                self.view = self.view.clamped(self.image_size(), s);
            }
            Msg::Zoom(factor) => self.view = self.view.zoomed(factor, Point::ZERO, self.image_size(), self.viewer),
            Msg::Fit => self.view = View { scale: None, pan: Point::ZERO, ..self.view },
            Msg::Actual => self.view = View { scale: Some(1.0), ..self.view }.clamped(self.image_size(), self.viewer),
            Msg::Turn(by) => self.view = View { turns: (self.view.turns + by) % 4, pan: Point::ZERO, ..self.view }.clamped(self.image_size(), self.viewer),
            Msg::Reveal => {
                if let Some(p) = self.path() {
                    let _ = reveal(p);
                }
            }
            Msg::Trash => {
                let Some(path) = self.path().cloned() else { return };
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match move_to_trash(&path) {
                    Ok(_) => {
                        self.files.remove(self.index);
                        self.index = self.index.min(self.files.len().saturating_sub(1));
                        self.current = None;
                        self.load();
                        self.status = Some((Tone::Good, format!("Moved {name} to the Trash.")));
                    }
                    Err(e) => self.status = Some((Tone::Bad, format!("Could not move {name} to the Trash: {e}"))),
                }
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        let has = self.current.is_some();
        let animated = self.current.as_ref().is_some_and(|d| !d.frames.is_empty());
        let many = self.files.len() > 1;
        let tool = |glyph, m: Msg, enabled: bool| icon_button(glyph, 34.0).kind(ButtonKind::Ghost).on_press_maybe(enabled.then_some(m));
        let scale = self.view.scale_in(self.image_size(), self.viewer);
        let zoom_label = if has { format!("{:.0}%", scale * 100.0) } else { "—".into() };
        let toolbar = row()
            .spacing(4.0)
            .align(Align::Center)
            .width(Length::Fill)
            .padding([12.0, 0.0])
            .push(tool(icons::CHEVRON_LEFT, Msg::Step(-1), many))
            .push(tool(icons::CHEVRON_RIGHT, Msg::Step(1), many))
            .push(Space::fill_x())
            .push(tool(icons::ZOOM_OUT, Msg::Zoom(1.0 / STEP), has))
            .push(Button::new(text(zoom_label).mono().role(TextRole::Caption)).kind(ButtonKind::Ghost).padding([8.0, 8.0]).width(64.0).on_press_maybe(has.then_some(if self.view.scale.is_none() { Msg::Actual } else { Msg::Fit })))
            .push(tool(icons::ZOOM_IN, Msg::Zoom(STEP), has))
            .push(tool(icons::MAXIMIZE, Msg::Fit, has && self.view.scale.is_some()))
            .push(Space::new(8.0, 0.0))
            .push_if(animated, || icon_button(if self.playing { icons::PAUSE } else { icons::PLAY }, 34.0).kind(ButtonKind::Ghost).on_press(Msg::PlayPause).into())
            .push_if(animated, || Space::new(8.0, 0.0).into())
            .push(tool(icons::ROTATE_CCW, Msg::Turn(3), has))
            .push(tool(icons::ROTATE_CW, Msg::Turn(1), has))
            .push(Space::new(8.0, 0.0))
            .push(tool(icons::FOLDER_SEARCH, Msg::Reveal, self.path().is_some()))
            .push(tool(icons::TRASH_2, Msg::Trash, self.path().is_some()));

        let body: Element<Msg> = match (&self.current, &self.error) {
            (Some(d), _) => Element::new(Viewer { image: d.frames.get(self.frame).map_or(&d.image, |f| &f.image).clone(), view: self.view }),
            (None, Some(e)) => message(icons::IMAGE_OFF, "This picture can't be shown", e.clone()),
            (None, None) if self.loading.is_some() => message(icons::IMAGE, "Opening…", String::new()),
            (None, None) => message(icons::IMAGES, "No picture open", "Open a picture from Files, or start Photos with a file or folder.".into()),
        };

        let mut info = Vec::new();
        if let (Some(d), Some(p)) = (&self.current, self.path()) {
            info.push(format!("{} × {}", d.width, d.height));
            if !d.frames.is_empty() {
                info.push(if d.truncated { format!("first {} frames", d.frames.len()) } else { format!("{} frames", d.frames.len()) });
            }
            if let Ok(meta) = std::fs::metadata(p) {
                info.push(human_size(meta.len()));
            }
        }
        if !self.files.is_empty() {
            info.push(format!("{} of {}", self.index + 1, self.files.len()));
        }
        let mut status = row().spacing(12.0).align(Align::Center).width(Length::Fill).padding([16.0, 8.0]).push(text(info.join("  ·  ")).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x());
        if let Some((tone, msg)) = &self.status {
            status = status.push(notice(*tone, msg.clone()));
        }

        let pane = container(column().width(Length::Fill).height(Length::Fill).push(toolbar).push(Divider::horizontal()).push(body).push(Divider::horizontal()).push(status)).surface(Surface::Card).width(Length::Fill).height(Length::Fill);
        container(pane).padding([0.0, 12.0, 12.0, 12.0]).width(Length::Fill).height(Length::Fill).into()
    }
}

fn message(glyph: neo::theme::Icon, title: &str, detail: String) -> Element<Msg> {
    let mut col = column().spacing(8.0).align(Align::Center).push(icon(glyph).size(36.0).tone(Tone::Faint)).push(text(title).role(TextRole::Strong));
    if !detail.is_empty() {
        col = col.push(container(text(detail).tone(Tone::Muted).align(Align::Center)).max_width(420.0));
    }
    container(col).width(Length::Fill).height(Length::Fill).center().into()
}

#[derive(Default)]
struct ViewerState {
    size: Size,
    /// Where the pointer was at the last drag step.
    dragging: Option<Point>,
    last_click: Option<Instant>,
}

/// Shows the picture and turns scrolling, pinching and dragging into view changes.
struct Viewer {
    image: Image,
    view: View,
}

impl Viewer {
    fn image_size(&self) -> Size {
        Size::new(self.image.width() as f32, self.image.height() as f32)
    }
}

impl Widget<Msg> for Viewer {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let st = cx.state::<ViewerState>();
        if st.size != size {
            st.size = size;
            cx.defer(Msg::ViewerSize(size));
        }
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        cx.scene.fill(b, 0.0, p.well, None);
        let r = self.view.rect(self.image_size(), b.size()).translate(b.origin());
        cx.scene.push_clip(b);
        cx.scene.image_turned(&self.image, r, self.view.turns);
        cx.scene.pop_clip();
        cx.scene.push_layer();
    }

    fn event(&mut self, cx: &mut EventCx<Msg>, event: &Event) -> Status {
        let b = cx.bounds();
        let (image, viewer) = (self.image_size(), b.size());
        let from_centre = |p: Point| Point::new(p.x - b.center().x, p.y - b.center().y);
        match event {
            Event::Wheel { pos, delta } if b.contains(*pos) => {
                // Scrolling up zooms in.
                cx.emit(Msg::View(self.view.zoomed((-delta.y * 0.0035).exp(), from_centre(*pos), image, viewer)));
                Status::Captured
            }
            Event::Pinch { pos, factor } if b.contains(*pos) => {
                cx.emit(Msg::View(self.view.zoomed(*factor, from_centre(*pos), image, viewer)));
                Status::Captured
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                let now = Instant::now();
                let st = cx.state::<ViewerState>();
                let double = st.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
                st.last_click = if double { None } else { Some(now) };
                st.dragging = Some(*pos);
                if double {
                    // Between fitting the window and actual size, about the click.
                    let next = if self.view.scale.is_none() { View { scale: Some(1.0), ..self.view }.zoomed(1.0, from_centre(*pos), image, viewer) } else { View { scale: None, pan: Point::ZERO, ..self.view } };
                    cx.emit(Msg::View(next));
                }
                Status::Captured
            }
            Event::PointerMoved { pos } => {
                let zoomed = self.view.scale.is_some();
                if b.contains(*pos) && zoomed {
                    cx.set_cursor(CursorIcon::Grab);
                }
                let st = cx.state::<ViewerState>();
                if let Some(last) = st.dragging {
                    st.dragging = Some(*pos);
                    if zoomed {
                        cx.set_cursor(CursorIcon::Grabbing);
                        let pan = Point::new(self.view.pan.x + pos.x - last.x, self.view.pan.y + pos.y - last.y);
                        cx.emit(Msg::View(View { pan, ..self.view }));
                    }
                }
                Status::Ignored
            }
            Event::PointerReleased { button: PointerButton::Primary, .. } | Event::PointerLeft => {
                cx.state::<ViewerState>().dragging = None;
                Status::Ignored
            }
            _ => Status::Ignored,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    let path = args.first().map(PathBuf::from).map(|p| std::path::absolute(&p).unwrap_or(p));
    if let Some(p) = &path
        && !p.exists()
    {
        eprintln!("neo-photos: {} does not exist", p.display());
        std::process::exit(2);
    }
    if let Err(e) = neo::run(Photos::new(path)) {
        eprintln!("neo-photos: {e}");
        std::process::exit(1);
    }
}

/// A picture to show in screenshots and tests: hills under a sky.
fn sample(width: u32, height: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(width, height, |x, y| {
        let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
        let ridge = 0.55 + 0.12 * (u * 9.0).sin() + 0.05 * (u * 23.0).cos();
        let far = 0.45 + 0.08 * (u * 5.0 + 1.0).sin();
        let sun = ((u - 0.72).powi(2) + (v - 0.28).powi(2)).sqrt();
        let c = if v > ridge {
            [38.0 + 30.0 * (1.0 - v), 92.0 + 40.0 * (1.0 - v), 74.0]
        } else if v > far {
            [84.0, 120.0 + 30.0 * v, 150.0]
        } else if sun < 0.06 {
            [255.0, 236.0, 170.0]
        } else {
            [110.0 + 120.0 * v, 150.0 + 80.0 * v, 220.0 - 30.0 * v]
        };
        image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255])
    })
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    let pictures = std::env::temp_dir().join(format!("neo-photos-snapshot-{}", std::process::id()));
    std::fs::create_dir_all(&pictures).expect("create sample dir");
    for (name, w, h) in [("Hills at noon.png", 1600, 1000), ("Hills, tall.png", 900, 1400), ("Hills, wide.png", 2400, 900)] {
        sample(w, h).save(pictures.join(name)).expect("write sample");
    }
    for (name, scheme, zoomed) in [("photos-fit", neo_desktop::SchemePref::Light, false), ("photos-zoomed", neo_desktop::SchemePref::Dark, true)] {
        let mut app = Photos::new(Some(pictures.join("Hills at noon.png")));
        app.desktop.appearance.scheme = scheme;
        let mut h = Harness::new(app, Size::new(1040.0, 720.0)).expect("GPU");
        // Decoding happens on a thread; wait for its answer.
        for _ in 0..100 {
            if h.app().current.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            h.advance(std::time::Duration::from_millis(20));
        }
        h.render(1.0);
        if zoomed {
            h.app_mut().update(Msg::Actual);
            h.app_mut().update(Msg::Turn(1));
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
    let _ = std::fs::remove_dir_all(pictures);
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE: Size = Size { w: 2000.0, h: 1000.0 };
    const VIEWER: Size = Size { w: 1000.0, h: 800.0 };

    #[test]
    fn fits_without_magnifying() {
        assert_eq!(View::FIT.rect(IMAGE, VIEWER), Rect::new(0.0, 150.0, 1000.0, 500.0));
        // A small picture is shown at its own size, centred.
        assert_eq!(View::FIT.rect(Size::new(200.0, 100.0), VIEWER), Rect::new(400.0, 350.0, 200.0, 100.0));
        // Turned on its side, the height is what has to fit.
        let turned = View { turns: 1, ..View::FIT };
        assert_eq!(turned.rect(IMAGE, VIEWER), Rect::new(300.0, 0.0, 400.0, 800.0));
    }

    #[test]
    fn zooms_about_the_pointer() {
        // Zooming in about a point leaves that point of the picture in place.
        let about = Point::new(200.0, -100.0);
        let before = View::FIT.rect(IMAGE, VIEWER);
        let v = View::FIT.zoomed(2.0, about, IMAGE, VIEWER);
        assert_eq!(v.scale, Some(1.0));
        let after = v.rect(IMAGE, VIEWER);
        let at = Point::new(VIEWER.w * 0.5 + about.x, VIEWER.h * 0.5 + about.y);
        let (fx, fy) = ((at.x - before.x) / before.w, (at.y - before.y) / before.h);
        assert!(((at.x - after.x) / after.w - fx).abs() < 1e-4 && ((at.y - after.y) / after.h - fy).abs() < 1e-4);
        // Zooming back out returns to fitting, and never past the maximum in.
        assert_eq!(v.zoomed(0.4, Point::ZERO, IMAGE, VIEWER), View::FIT);
        assert_eq!(v.zoomed(1000.0, Point::ZERO, IMAGE, VIEWER).scale, Some(MAX_SCALE));
    }

    #[test]
    fn cannot_be_dragged_out_of_sight() {
        let v = View { scale: Some(1.0), pan: Point::new(5000.0, 5000.0), turns: 0 }.clamped(IMAGE, VIEWER);
        // 2000 wide in a 1000 viewer: half the overflow each way. 1000 tall in 800: 100.
        assert_eq!(v.pan, Point::new(500.0, 100.0));
        assert_eq!(View { pan: Point::new(40.0, 40.0), ..View::FIT }.clamped(IMAGE, VIEWER).pan, Point::ZERO, "a picture that fits stays centred");
    }

    #[test]
    fn animations_step_through_their_frames_and_pause() {
        use image::codecs::gif::GifEncoder;
        let dir = std::env::temp_dir().join(format!("neo-photos-play-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spin.gif");
        {
            let mut encoder = GifEncoder::new(std::fs::File::create(&path).unwrap());
            for (shade, ms) in [(0u8, 50), (128, 300)] {
                encoder.encode_frame(image::Frame::from_parts(image::RgbaImage::from_pixel(6, 6, image::Rgba([shade, 0, 0, 255])), 0, 0, image::Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let mut app = Photos::new(Some(path));
        app.load();
        let timer = |a: &Photos| a.subscriptions().iter().map(|s| s.period).find(|p| *p != std::time::Duration::from_millis(750));
        assert_eq!(timer(&app), Some(std::time::Duration::from_millis(50)), "the first frame's own delay");
        app.update(Msg::Advance);
        assert_eq!((app.frame, timer(&app)), (1, Some(std::time::Duration::from_millis(300))));
        app.update(Msg::Advance);
        assert_eq!(app.frame, 0, "the animation loops");
        app.update(Msg::PlayPause);
        assert_eq!(timer(&app), None, "paused: no timer");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn walks_the_folder_in_file_manager_order() {
        let dir = std::env::temp_dir().join(format!("neo-photos-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["shot10.png", "shot2.png", "Shot1.PNG", "notes.txt"] {
            sample(8, 8).save_with_format(dir.join(name), image::ImageFormat::Png).unwrap();
        }
        let mut app = Photos::new(Some(dir.join("shot2.png")));
        let names = |a: &Photos| a.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert_eq!(names(&app), ["Shot1.PNG", "shot2.png", "shot10.png"], "pictures only, in natural order");
        assert_eq!(app.index, 1);
        app.load();
        assert_eq!(app.current.as_ref().map(|d| d.width), Some(8));
        app.update(Msg::Step(1));
        assert_eq!(app.index, 2);
        app.update(Msg::Step(1));
        assert_eq!(app.index, 0, "stepping past the end wraps round");
        // Opening the folder itself starts at its first picture.
        assert_eq!(Photos::new(Some(dir.clone())).index, 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
