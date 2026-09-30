use neo_render::{Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Surface, TextRole};

use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, PointerButton, Status};

#[derive(Default)]
struct SegState {
    hovered: Option<usize>,
    pressed: Option<usize>,
}

/// A row of mutually exclusive options.
pub struct Segmented<M> {
    options: Vec<String>,
    selected: Option<usize>,
    on_select: Box<dyn Fn(usize) -> M>,
    width: Length,
    layouts: Vec<TextLayout>,
    rects: Vec<Rect>,
}

impl<M> Segmented<M> {
    pub fn new<S: Into<String>>(options: impl IntoIterator<Item = S>, selected: Option<usize>, on_select: impl Fn(usize) -> M + 'static) -> Self {
        Self {
            options: options.into_iter().map(Into::into).collect(),
            selected,
            on_select: Box::new(on_select),
            width: Length::Shrink,
            layouts: vec![],
            rects: vec![],
        }
    }

    /// `Fill` spreads the options evenly across the available width.
    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }
}

/// Shorthand for [`Segmented::new`].
pub fn segmented<M, S: Into<String>>(options: impl IntoIterator<Item = S>, selected: Option<usize>, on_select: impl Fn(usize) -> M + 'static) -> Segmented<M> {
    Segmented::new(options, selected, on_select)
}

const PAD: f32 = 4.0;
const GAP: f32 = 4.0;
const OPT_X: f32 = 14.0;
const OPT_Y: f32 = 7.0;

impl<M: 'static> Widget<M> for Segmented<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let mut style = TextStyle::from_spec(cx.theme().text(TextRole::Strong));
        style.size *= 0.93;
        self.layouts = self.options.iter().map(|o| cx.text().layout(o, &style, None)).collect();
        let n = self.layouts.len().max(1) as f32;
        let text_h = self.layouts.iter().map(|l| l.size().h).fold(0.0, f32::max);
        let natural: Vec<f32> = self.layouts.iter().map(|l| l.size().w + OPT_X * 2.0).collect();
        let natural_w = natural.iter().sum::<f32>() + GAP * (n - 1.0) + PAD * 2.0;
        let l = limits.constrain(self.width, Length::Shrink);
        let size = l.resolve(Size::new(natural_w, text_h + OPT_Y * 2.0 + PAD * 2.0));
        let even = self.width.is_fill();
        let slot = (size.w - PAD * 2.0 - GAP * (n - 1.0)) / n;
        let mut x = PAD;
        self.rects = natural
            .iter()
            .map(|w| {
                let w = if even { slot } else { *w };
                let r = Rect::new(x, PAD, w, size.h - PAD * 2.0);
                x += w + GAP;
                r
            })
            .collect();
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let outer_r = theme.small_radius() + PAD;
        cx.scene.paint(b, outer_r, &theme.paint(Surface::Inset));
        cx.focus_ring(b, outer_r);
        let (hovered, pressed) = {
            let st = cx.state::<SegState>();
            (st.hovered, st.pressed)
        };
        for (i, (r, l)) in self.rects.iter().zip(&self.layouts).enumerate() {
            let r = r.translate(b.origin());
            let selected = self.selected == Some(i) || pressed == Some(i);
            let color = if selected {
                let mut paint = theme.paint(Surface::Pressed);
                paint.content = p.accent_text;
                cx.scene.paint(r, theme.small_radius(), &paint);
                p.accent_text
            } else if hovered == Some(i) {
                p.text
            } else {
                p.muted
            };
            let s = l.size();
            cx.scene.text(l, Point::new(r.x + ((r.w - s.w) * 0.5).round(), r.y + ((r.h - s.h) * 0.5).round()), color);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let hit = |p: Point| self.rects.iter().position(|r| r.translate(b.origin()).contains(p));
        match event {
            Event::PointerMoved { pos } => {
                let h = hit(*pos);
                let st = cx.state::<SegState>();
                if st.hovered != h {
                    st.hovered = h;
                    cx.request_redraw();
                }
                if h.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<SegState>().hovered = None;
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } => match hit(*pos) {
                Some(i) => {
                    cx.state::<SegState>().pressed = Some(i);
                    cx.request_redraw();
                    Status::Captured
                }
                None => Status::Ignored,
            },
            Event::PointerReleased { pos, .. } => {
                let pressed = cx.state::<SegState>().pressed.take();
                if let Some(i) = pressed {
                    cx.request_redraw();
                    if hit(*pos) == Some(i) && self.selected != Some(i) {
                        cx.emit((self.on_select)(i));
                    }
                }
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let n = self.options.len();
                let cur = self.selected.unwrap_or(0);
                let next = match k.key {
                    Key::Left | Key::Up => cur.checked_sub(1),
                    Key::Right | Key::Down => (cur + 1 < n).then_some(cur + 1),
                    _ => return Status::Ignored,
                };
                if let Some(i) = next {
                    cx.emit((self.on_select)(i));
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
