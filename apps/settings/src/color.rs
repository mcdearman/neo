//! Choosing a colour: by eye from a plane of shades, by its hex code, or
//! straight off the screen with an eyedropper.

use neo::prelude::*;
use neo::{CursorIcon, Cx, DrawCx, Event, EventCx, Image, Limits, Point, PointerButton, Rect, Size, Status, Widget};

/// A colour as hue, saturation and value, each from 0 to 1: the way the
/// picker lays colours out. Hue is around the wheel, saturation is how
/// much colour against grey, value is how light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn of(color: Color) -> Self {
        let (r, g, b) = (color.r, color.g, color.b);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let spread = max - min;
        let h = if spread <= f32::EPSILON {
            0.0
        } else if max == r {
            ((g - b) / spread).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / spread + 2.0) / 6.0
        } else {
            ((r - g) / spread + 4.0) / 6.0
        };
        Self { h, s: if max <= f32::EPSILON { 0.0 } else { spread / max }, v: max }
    }

    pub fn color(self) -> Color {
        let (h, s, v) = (self.h.rem_euclid(1.0) * 6.0, self.s.clamp(0.0, 1.0), self.v.clamp(0.0, 1.0));
        let (c, x) = (v * s, v * s * (1.0 - (h % 2.0 - 1.0).abs()));
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        Color::rgb(r + m, g + m, b + m)
    }
}

/// A colour's code: `#3F5BC4`.
pub fn hex(color: Color) -> String {
    let [r, g, b, _] = color.to_rgba8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Reads a colour's code, with or without its `#`, in six digits or the
/// short three: `#3F5BC4`, `3f5bc4`, `#35c`. `None` until it is one.
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full: String = match digits.len() {
        6 => digits.to_owned(),
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        _ => return None,
    };
    u32::from_str_radix(&full, 16).ok().map(Color::hex)
}

/// Every shade of one hue: more colour to the right, lighter to the top.
pub fn shades(hue: f32) -> Image {
    let (w, h) = (96u32, 64u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend(Hsv { h: hue, s: x as f32 / (w - 1) as f32, v: 1.0 - y as f32 / (h - 1) as f32 }.color().to_rgba8());
        }
    }
    Image::new(w, h, rgba)
}

/// Every hue, side by side.
pub fn hues() -> Image {
    let w = 180u32;
    let rgba = (0..w).flat_map(|x| Hsv { h: x as f32 / w as f32, s: 1.0, v: 1.0 }.color().to_rgba8()).collect();
    Image::new(w, 1, rgba)
}

#[derive(Default)]
struct PlaneState {
    dragging: bool,
}

/// A picture to pick a place in by pressing or dragging: the plane of
/// shades, or the strip of hues. A ring marks the place now picked.
pub struct Plane<M> {
    image: Image,
    /// Where the mark is, each from 0 to 1 across and down.
    at: (f32, f32),
    size: Size,
    on_pick: Box<dyn Fn(f32, f32) -> M>,
}

impl<M> Plane<M> {
    pub fn new(image: Image, at: (f32, f32), size: Size, on_pick: impl Fn(f32, f32) -> M + 'static) -> Self {
        Self { image, at, size, on_pick: Box::new(on_pick) }
    }

    fn place(&self, bounds: Rect, p: Point) -> (f32, f32) {
        (((p.x - bounds.x) / bounds.w.max(1.0)).clamp(0.0, 1.0), ((p.y - bounds.y) / bounds.h.max(1.0)).clamp(0.0, 1.0))
    }
}

impl<M: 'static> Widget<M> for Plane<M> {
    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        Size::new(self.size.w.min(limits.max.w), self.size.h.min(limits.max.h))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let line = cx.theme().palette().line;
        cx.scene.push_clip(b);
        cx.scene.image(&self.image, b);
        cx.scene.pop_clip();
        // Over the picture, which is drawn above the shapes of its own layer.
        cx.scene.push_layer();
        cx.scene.fill(b, 0.0, Color::TRANSPARENT, Some((1.0, line)));
        let at = Point::new((b.x + self.at.0 * b.w).round(), (b.y + self.at.1 * b.h).round());
        // A ring that shows on any colour: white inside dark.
        let r = (b.h * 0.5).min(7.0);
        cx.scene.fill(Rect::new(at.x - r - 1.0, at.y - r - 1.0, (r + 1.0) * 2.0, (r + 1.0) * 2.0), r + 1.0, Color::TRANSPARENT, Some((1.0, Color::BLACK.with_alpha(0.55))));
        cx.scene.fill(Rect::new(at.x - r, at.y - r, r * 2.0, r * 2.0), r, Color::TRANSPARENT, Some((2.0, Color::WHITE)));
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.state::<PlaneState>().dragging = true;
                let (x, y) = self.place(b, *pos);
                cx.emit((self.on_pick)(x, y));
                Status::Captured
            }
            Event::PointerMoved { pos } => {
                let dragging = cx.state::<PlaneState>().dragging;
                if dragging || b.contains(*pos) {
                    cx.set_cursor(CursorIcon::Crosshair);
                }
                if dragging {
                    let (x, y) = self.place(b, *pos);
                    cx.emit((self.on_pick)(x, y));
                }
                Status::Ignored
            }
            Event::PointerReleased { .. } => {
                cx.state::<PlaneState>().dragging = false;
                Status::Ignored
            }
            _ => Status::Ignored,
        }
    }
}

/// Whether a colour can be picked off the screen on this system.
pub fn can_sample() -> bool {
    imp::can_sample()
}

/// Brings up the eyedropper, and gives `picked` the colour clicked on,
/// or nothing if it was dismissed. To be called on the main thread.
pub fn sample(picked: impl Fn(Option<Color>) + Send + Sync + 'static) {
    imp::sample(Box::new(picked));
}

type Picked = Box<dyn Fn(Option<Color>) + Send + Sync>;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::sync::Mutex;

    /// Who to tell of the colour the eyedropper comes back with.
    static WAITING: Mutex<Option<Picked>> = Mutex::new(None);

    unsafe extern "C" {
        fn neo_sample_color(callback: extern "C" fn(std::ffi::c_int, f64, f64, f64));
    }

    extern "C" fn sampled(ok: std::ffi::c_int, r: f64, g: f64, b: f64) {
        if let Some(tell) = WAITING.lock().ok().and_then(|mut w| w.take()) {
            tell((ok != 0).then(|| Color::rgb(r as f32, g as f32, b as f32)));
        }
    }

    pub fn can_sample() -> bool {
        true
    }

    pub fn sample(picked: Picked) {
        if let Ok(mut waiting) = WAITING.lock() {
            *waiting = Some(picked);
        }
        // SAFETY: called on the main thread, with a function that lives as long as the program.
        unsafe { neo_sample_color(sampled) };
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// The pickers there are on Linux desktops: each prints the colour's code.
    const TOOLS: [(&str, &[&str]); 2] = [("hyprpicker", &["-n"]), ("xcolor", &[])];

    fn tool() -> Option<(std::path::PathBuf, &'static [&'static str])> {
        TOOLS.iter().map(|(name, args)| (neo_desktop::fs::tool(name), *args)).find(|(path, _)| path.is_file())
    }

    pub fn can_sample() -> bool {
        tool().is_some()
    }

    pub fn sample(picked: Picked) {
        let Some((program, args)) = tool() else { return picked(None) };
        std::thread::spawn(move || {
            let out = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
            picked(out.ok().filter(|o| o.status.success()).and_then(|o| parse_hex(String::from_utf8_lossy(&o.stdout).trim())));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_goes_to_hue_and_shade_and_back() {
        for code in [0x3F5BC4, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFD866, 0x101820, 0xFFFFFF, 0x000000, 0x808080, 0xFF2D95] {
            let color = Color::hex(code);
            assert_eq!(hex(Hsv::of(color).color()), hex(color), "{code:06X}");
        }
        let red = Hsv::of(Color::hex(0xFF0000));
        assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
        let grey = Hsv::of(Color::hex(0x808080));
        assert!(grey.s == 0.0 && (grey.v - 0.502).abs() < 0.01, "no colour in it, half as light");
        assert!((Hsv::of(Color::hex(0x00FF00)).h - 1.0 / 3.0).abs() < 0.001 && (Hsv::of(Color::hex(0x0000FF)).h - 2.0 / 3.0).abs() < 0.001);
        // A hue a whole turn on is the same hue.
        assert_eq!(hex(Hsv { h: 1.25, s: 1.0, v: 1.0 }.color()), hex(Hsv { h: 0.25, s: 1.0, v: 1.0 }.color()));
    }

    #[test]
    fn a_colours_code_is_written_and_read() {
        assert_eq!(hex(Color::hex(0x3F5BC4)), "#3F5BC4");
        assert_eq!(parse_hex("#3F5BC4"), Some(Color::hex(0x3F5BC4)));
        assert_eq!(parse_hex(" 3f5bc4 "), Some(Color::hex(0x3F5BC4)));
        assert_eq!(parse_hex("#35c"), Some(Color::hex(0x3355CC)), "the short form");
        for not_yet in ["", "#", "#3F5B", "3F5BC4A", "#GGGGGG", "blue", "#3F 5BC4"] {
            assert_eq!(parse_hex(not_yet), None, "{not_yet:?}");
        }
    }

    #[test]
    fn the_pictures_to_pick_from_are_of_what_they_say() {
        let plane = shades(0.0);
        assert_eq!((plane.width(), plane.height()), (96, 64));
        // Every hue across the strip: red at the start, and not red a third of the way along.
        assert_eq!(hues().width(), 180);
        let (top_right, bottom, top_left) = (Hsv { h: 0.0, s: 1.0, v: 1.0 }.color(), Hsv { h: 0.0, s: 1.0, v: 0.0 }.color(), Hsv { h: 0.0, s: 0.0, v: 1.0 }.color());
        assert_eq!((hex(top_right), hex(bottom), hex(top_left)), ("#FF0000".into(), "#000000".into(), "#FFFFFF".into()), "full colour, black, white at the corners");
    }
}
