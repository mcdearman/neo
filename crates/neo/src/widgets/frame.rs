//! The window frame: background, rounded corners and, with Neo decorations,
//! the title bar with window controls.

use std::time::Instant;

use neo_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::style::lerp_paint;
use crate::anim::Anim;
use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Limits, Widget, WindowRequest};
use crate::event::{Event, PointerButton, Status};

pub(crate) const TITLE_H: f32 = 46.0;
const CTL: f32 = 22.0;
const CTL_GAP: f32 = 9.0;

#[derive(Default)]
struct FrameState {
    hovered: Option<usize>,
    pressed: Option<usize>,
    last_click: Option<Instant>,
    anims: [Anim; 3],
}

pub(crate) struct Frame<M> {
    content: [Element<M>; 1],
    title: String,
    chrome: bool,
    rounded: bool,
    /// Paint the window background. Off for see-through windows.
    background: bool,
    title_layout: Option<TextLayout>,
}

impl<M: 'static> Frame<M> {
    pub fn new(content: Element<M>, title: String, chrome: bool, rounded: bool, background: bool) -> Self {
        Self { content: [content], title, chrome, rounded, background, title_layout: None }
    }

    fn controls(&self, b: Rect) -> [Rect; 3] {
        let y = b.y + (TITLE_H - CTL) * 0.5;
        let right = b.right() - 16.0;
        [2.0, 1.0, 0.0].map(|i| Rect::new(right - CTL - i * (CTL + CTL_GAP), y, CTL, CTL))
    }
}

impl<M: 'static> Widget<M> for Frame<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.content
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let top = if self.chrome { TITLE_H } else { 0.0 };
        if self.chrome {
            let spec = cx.theme().text(TextRole::Strong);
            self.title_layout = Some(cx.text().layout(&self.title, &spec.style(), None));
        }
        self.content[0].layout(cx, Limits::tight(Size::new(size.w, (size.h - top).max(0.0))));
        self.content[0].set_position(Point::new(0.0, top));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let radius = if self.rounded { theme.window_radius() } else { 0.0 };
        let mut window = theme.paint(Surface::Window);
        if self.rounded {
            window.border = Some((1.0, p.line.with_alpha(if theme.glass.enabled { 0.5 } else { 1.0 })));
        }
        if self.background {
            cx.scene.paint(b, radius, &window);
        }

        if self.chrome {
            if let Some(l) = &self.title_layout {
                let y = b.y + ((TITLE_H - l.size().h) * 0.5).round();
                cx.scene.text(l, Point::new(b.x + 20.0, y), p.text);
            }
            let now = cx.now();
            let motion = theme.motion();
            let (hovered, pressed) = {
                let st = cx.state::<FrameState>();
                (st.hovered, st.pressed)
            };
            let mut animating = false;
            for (i, r) in self.controls(b).iter().enumerate() {
                let t = {
                    let st = cx.state::<FrameState>();
                    let t = st.anims[i].step(if hovered == Some(i) { 1.0 } else { 0.0 }, now, motion);
                    animating |= st.anims[i].is_animating();
                    t
                };
                let mut paint = lerp_paint(&theme.paint(Surface::Raised), &theme.paint(Surface::Hovered), t);
                if pressed == Some(i) {
                    paint = theme.paint(Surface::Pressed);
                }
                let fg = if i == 2 && t > 0.0 { p.muted.mix(p.bad, t) } else { p.faint.mix(p.text, t * 0.6) };
                cx.scene.paint(*r, CTL * 0.5, &paint);
                let c = r.center();
                let s = 3.5;
                match i {
                    0 => cx.scene.line(Point::new(c.x - s, c.y), Point::new(c.x + s, c.y), 1.8, fg),
                    1 => cx.scene.fill(Rect::new(c.x - s, c.y - s, s * 2.0, s * 2.0), 1.5, neo_theme::Color::TRANSPARENT, Some((1.6, fg))),
                    _ => {
                        cx.scene.line(Point::new(c.x - s, c.y - s), Point::new(c.x + s, c.y + s), 1.8, fg);
                        cx.scene.line(Point::new(c.x - s, c.y + s), Point::new(c.x + s, c.y - s), 1.8, fg);
                    }
                }
            }
            if animating {
                cx.request_animation();
            }
        }
        cx.with_content_color(p.text, |cx| self.content[0].draw(cx));
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        if self.chrome {
            let bar = Rect::new(b.x, b.y, b.w, TITLE_H);
            let ctl = self.controls(b);
            let hit = |p: Point| ctl.iter().position(|r| r.inset(-3.0).contains(p));
            match event {
                Event::PointerMoved { pos } => {
                    let h = hit(*pos);
                    let st = cx.state::<FrameState>();
                    if st.hovered != h {
                        st.hovered = h;
                        cx.request_redraw();
                    }
                    if h.is_some() {
                        cx.set_cursor(CursorIcon::Pointer);
                    }
                }
                Event::PointerLeft => {
                    cx.state::<FrameState>().hovered = None;
                }
                Event::PointerPressed { pos, button: PointerButton::Primary } if bar.contains(*pos) => {
                    if let Some(i) = hit(*pos) {
                        cx.state::<FrameState>().pressed = Some(i);
                        cx.request_redraw();
                        return Status::Captured;
                    }
                    let now = Instant::now();
                    let st = cx.state::<FrameState>();
                    let double = st.last_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
                    st.last_click = Some(now);
                    cx.window_request(if double { WindowRequest::ToggleMaximize } else { WindowRequest::Drag });
                    return Status::Captured;
                }
                Event::PointerReleased { pos, .. } => {
                    let pressed = cx.state::<FrameState>().pressed.take();
                    if let Some(i) = pressed {
                        cx.request_redraw();
                        if hit(*pos) == Some(i) {
                            cx.window_request(match i {
                                0 => WindowRequest::Minimize,
                                1 => WindowRequest::ToggleMaximize,
                                _ => WindowRequest::Close,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        self.content[0].event(cx, event)
    }
}
