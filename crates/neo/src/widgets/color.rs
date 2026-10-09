//! Choosing a colour by eye: a plane of one hue's shades and a strip of
//! every hue to pick from, and a colour's hex code written and read.

use armature_render::{Color, Image, Point, Rect, Size};

use crate::ThemeCx;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Limits, Widget};
use crate::event::{Event, PointerButton, Status};

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

/// A colour to change: a swatch of it and its code, which opens where it
/// stands into a plane of its hue's shades and a strip of every hue to
/// pick from. See [`color_field`].
pub struct ColorField<M> {
    color: Color,
    on_change: Option<Box<dyn Fn(Color) -> M>>,
    on_scrub: Option<Box<dyn Fn(bool) -> M>>,
    width: crate::core::Length,
    code: Option<armature_render::TextLayout>,
    open: bool,
}

/// A field showing `color`, opened with a click to pick another. The
/// colour is as a hex code means it: sRGB, with no part of it see-through.
pub fn color_field<M>(color: Color) -> ColorField<M> {
    ColorField { color, on_change: None, on_scrub: None, width: crate::core::Length::Fill, code: None, open: false }
}

/// What a colour field keeps while it is open.
#[derive(Default)]
struct FieldState {
    open: bool,
    /// The colour as it is being picked: kept apart from the colour
    /// itself, which forgets its hue when it is grey or black.
    hsv: Option<Hsv>,
    /// The plane for the hue it was last made for, and the strip of hues.
    plane: Option<(f32, Image)>,
    strip: Option<Image>,
    /// Dragging in the plane (false) or along the strip (true).
    dragging: Option<bool>,
    /// A code being typed over the one shown, and whether the first key is still to replace it.
    typing: Option<(String, bool)>,
}

const FIELD_H: f32 = 28.0;
const PLANE_H: f32 = 120.0;
const STRIP_H: f32 = 14.0;
const FIELD_GAP: f32 = 8.0;

impl<M> ColorField<M> {
    /// Told the colour as it changes: at every move of a drag, and when a code is entered.
    pub fn on_change(mut self, f: impl Fn(Color) -> M + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Told when a drag in the plane or along the strip begins (true)
    /// and ends, for all the changes between to be one change to undo.
    pub fn on_scrub(mut self, f: impl Fn(bool) -> M + 'static) -> Self {
        self.on_scrub = Some(Box::new(f));
        self
    }

    pub fn width(mut self, w: impl Into<crate::core::Length>) -> Self {
        self.width = w.into();
        self
    }

    fn head(&self, b: Rect) -> Rect {
        Rect::new(b.x, b.y, b.w, FIELD_H)
    }

    fn plane(&self, b: Rect) -> Rect {
        Rect::new(b.x, b.y + FIELD_H + FIELD_GAP, b.w, PLANE_H)
    }

    fn strip(&self, b: Rect) -> Rect {
        Rect::new(b.x, b.y + FIELD_H + FIELD_GAP + PLANE_H + FIELD_GAP, b.w, STRIP_H)
    }

    fn say(&self, cx: &mut EventCx<M>, color: Color) {
        if let (Some(f), true) = (&self.on_change, hex(color) != hex(self.color)) {
            cx.emit(f(color));
        }
    }

    /// Picks at `p`, in the plane or along the strip.
    fn pick(&self, cx: &mut EventCx<M>, b: Rect, strip: bool, p: Point) {
        let mut hsv = cx.state::<FieldState>().hsv.unwrap_or_else(|| Hsv::of(self.color));
        if strip {
            let r = self.strip(b);
            hsv.h = ((p.x - r.x) / r.w.max(1.0)).clamp(0.0, 0.999);
        } else {
            let r = self.plane(b);
            (hsv.s, hsv.v) = (((p.x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0), 1.0 - ((p.y - r.y) / r.h.max(1.0)).clamp(0.0, 1.0));
        }
        cx.state::<FieldState>().hsv = Some(hsv);
        cx.request_layout();
        self.say(cx, hsv.color());
    }
}

impl<M: 'static> Widget<M> for ColorField<M> {
    fn width(&self) -> crate::core::Length {
        self.width
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = cx.theme().text(neo_theme::TextRole::Body).style();
        let color = self.color;
        let st = cx.state::<FieldState>();
        self.open = st.open;
        // Changed from elsewhere, the picking starts again from what it is now.
        if st.hsv.is_none_or(|h| hex(h.color()) != hex(color)) {
            st.hsv = Some(Hsv::of(color));
        }
        let hue = st.hsv.map_or(0.0, |h| h.h);
        if st.open {
            if st.plane.as_ref().is_none_or(|(h, _)| *h != hue) {
                st.plane = Some((hue, shades(hue)));
            }
            if st.strip.is_none() {
                st.strip = Some(hues());
            }
        }
        let shown = st.typing.as_ref().map_or_else(|| hex(color), |(t, _)| t.clone());
        self.code = Some(cx.text().layout(&shown, &style, None));
        let tall = if self.open { FIELD_H + FIELD_GAP + PLANE_H + FIELD_GAP + STRIP_H } else { FIELD_H };
        limits.constrain(self.width, crate::core::Length::Shrink).resolve(Size::new(180.0, tall))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let head = self.head(b);
        let (hsv, plane, strip, typing) = {
            let st = cx.state::<FieldState>();
            (st.hsv.unwrap_or_else(|| Hsv::of(self.color)), st.plane.as_ref().map(|(_, i)| i.clone()), st.strip.clone(), st.typing.clone())
        };
        cx.scene.paint(head, theme.small_radius(), &theme.paint(if self.open { neo_theme::Surface::Hovered } else { neo_theme::Surface::Inset }));
        if cx.focus_visible() {
            cx.scene.fill(head.inset(-3.0), theme.small_radius() + 3.0, Color::TRANSPARENT, Some((2.0, p.accent_text)));
        }
        let swatch = Rect::new(head.x + 5.0, head.y + 5.0, 30.0, head.h - 10.0);
        cx.scene.fill(swatch, 4.0, self.color.with_alpha(1.0), Some((1.0, p.line)));
        if let Some(code) = &self.code {
            let s = code.size();
            let at = Point::new(swatch.right() + 8.0, head.y + ((head.h - s.h) * 0.5).round());
            if let Some((_, fresh)) = &typing {
                if *fresh {
                    cx.scene.fill(Rect::new(at.x - 1.0, at.y, s.w + 2.0, s.h), 2.0, p.accent.with_alpha(0.3), None);
                } else {
                    cx.scene.fill(Rect::new(at.x + s.w + 1.0, at.y + 1.0, 1.5, s.h - 2.0), 0.0, p.text, None);
                }
            }
            cx.scene.text(code, at, p.text);
        }
        if !self.open {
            return;
        }
        // The plane and the strip, each with a ring where the colour is on it.
        let ring = |cx: &mut DrawCx, at: Point, r: f32| {
            cx.scene.fill(Rect::new(at.x - r - 1.0, at.y - r - 1.0, (r + 1.0) * 2.0, (r + 1.0) * 2.0), r + 1.0, Color::TRANSPARENT, Some((1.0, Color::BLACK.with_alpha(0.55))));
            cx.scene.fill(Rect::new(at.x - r, at.y - r, r * 2.0, r * 2.0), r, Color::TRANSPARENT, Some((2.0, Color::WHITE)));
        };
        let (pr, sr) = (self.plane(b), self.strip(b));
        for (image, r) in [(plane, pr), (strip, sr)] {
            if let Some(image) = image {
                cx.scene.push_clip(r);
                cx.scene.image(&image, r);
                cx.scene.pop_clip();
            }
        }
        // Over the pictures, which are drawn above the shapes of their own layer.
        cx.scene.push_layer();
        cx.scene.fill(pr, 0.0, Color::TRANSPARENT, Some((1.0, p.line)));
        cx.scene.fill(sr, 0.0, Color::TRANSPARENT, Some((1.0, p.line)));
        ring(cx, Point::new((pr.x + hsv.s * pr.w).round(), (pr.y + (1.0 - hsv.v) * pr.h).round()), 6.0);
        ring(cx, Point::new((sr.x + hsv.h.rem_euclid(1.0) * sr.w).round(), (sr.y + sr.h * 0.5).round()), 6.0);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let enter = |this: &Self, cx: &mut EventCx<M>| {
            if let Some((typed, _)) = cx.state::<FieldState>().typing.take() {
                if let Some(c) = parse_hex(&typed) {
                    cx.state::<FieldState>().hsv = Some(Hsv::of(c));
                    this.say(cx, c);
                }
                cx.request_layout();
            }
        };
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } if self.head(b).contains(*pos) => {
                cx.request_focus();
                // On the code: it is typed over. On the rest: opened, or shut.
                if pos.x > b.x + 40.0 && self.open {
                    cx.state::<FieldState>().typing = Some((hex(self.color), true));
                } else {
                    enter(self, cx);
                    let st = cx.state::<FieldState>();
                    st.open = !st.open;
                }
                cx.request_layout();
                Status::Captured
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if self.open && (self.plane(b).contains(*pos) || self.strip(b).contains(*pos)) => {
                cx.request_focus();
                enter(self, cx);
                let strip = self.strip(b).contains(*pos);
                cx.state::<FieldState>().dragging = Some(strip);
                if let Some(f) = &self.on_scrub {
                    cx.emit(f(true));
                }
                self.pick(cx, b, strip, *pos);
                Status::Captured
            }
            // A press anywhere else: a code half typed is entered.
            Event::PointerPressed { .. } => {
                enter(self, cx);
                Status::Ignored
            }
            Event::PointerMoved { pos } => {
                if let Some(strip) = cx.state::<FieldState>().dragging {
                    cx.set_cursor(CursorIcon::Crosshair);
                    self.pick(cx, b, strip, *pos);
                    return Status::Captured;
                }
                if self.open && (self.plane(b).contains(*pos) || self.strip(b).contains(*pos)) {
                    cx.set_cursor(CursorIcon::Crosshair);
                } else if self.head(b).contains(*pos) {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                Status::Ignored
            }
            Event::PointerReleased { .. } => {
                if cx.state::<FieldState>().dragging.take().is_some() {
                    if let Some(f) = &self.on_scrub {
                        cx.emit(f(false));
                    }
                    return Status::Captured;
                }
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let typing = cx.state::<FieldState>().typing.clone();
                match (&k.key, typing) {
                    (crate::event::Key::Enter, Some(_)) => enter(self, cx),
                    (crate::event::Key::Escape, Some(_)) => cx.state::<FieldState>().typing = None,
                    (crate::event::Key::Escape, None) if self.open => cx.state::<FieldState>().open = false,
                    (crate::event::Key::Enter, None) => {
                        let st = cx.state::<FieldState>();
                        st.open = !st.open;
                    }
                    (crate::event::Key::Backspace, Some((mut typed, fresh))) => {
                        if fresh {
                            typed.clear();
                        } else {
                            typed.pop();
                        }
                        cx.state::<FieldState>().typing = Some((typed, false));
                    }
                    (_, Some((typed, fresh))) => {
                        let Some(more) = k.text.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_hexdigit() || c == '#')) else { return Status::Ignored };
                        cx.state::<FieldState>().typing = Some((if fresh { more.to_owned() } else { typed + more }, false));
                    }
                    _ => return Status::Ignored,
                }
                cx.request_layout();
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

impl<M: 'static> From<ColorField<M>> for crate::core::Element<M> {
    fn from(w: ColorField<M>) -> Self {
        crate::core::Element::new(w)
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
