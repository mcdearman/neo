//! The window frame: background, rounded corners and, with Neo decorations,
//! the title bar with window controls. Where the system has no menu bar of
//! its own, the app's menus go here too.

use std::time::Instant;

use armature_render::{Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use super::menu::{Row, RowList};
use super::style::lerp_paint;
use crate::ThemeCx;
use crate::anim::Anim;
use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Limits, Widget, WindowRequest};
use crate::event::{Event, Key, PointerButton, Status};

pub(crate) const TITLE_H: f32 = 46.0;
/// Height of the menu bar when there is no title bar to put it in.
pub(crate) const MENU_H: f32 = 30.0;
const MENU_TITLE_H: f32 = 26.0;
const CTL: f32 = 22.0;
const CTL_GAP: f32 = 9.0;

#[derive(Default)]
struct FrameState {
    hovered: Option<usize>,
    pressed: Option<usize>,
    last_click: Option<Instant>,
    anims: [Anim; 3],
    /// The menu whose dropdown is showing, and the row under the pointer.
    menu_open: Option<usize>,
    menu_row: Option<usize>,
    menu_title: Option<usize>,
}

pub(crate) struct Frame<M> {
    content: [Element<M>; 1],
    title: String,
    chrome: bool,
    rounded: bool,
    /// Paint the window background. Off for see-through windows.
    background: bool,
    title_layout: Option<TextLayout>,
    menus: Vec<armature::Menu<M>>,
    menu_rows: Vec<Vec<Row>>,
    menu_lists: Vec<RowList>,
    /// Each menu's title and where it sits, relative to the window.
    menu_titles: Vec<(TextLayout, Rect)>,
}

impl<M: 'static> Frame<M> {
    pub fn new(content: Element<M>, title: String, chrome: bool, rounded: bool, background: bool) -> Self {
        Self { content: [content], title, chrome, rounded, background, title_layout: None, menus: vec![], menu_rows: vec![], menu_lists: vec![], menu_titles: vec![] }
    }

    /// The app's menus, to draw at the top of the window.
    pub fn menus(mut self, menus: Vec<armature::Menu<M>>) -> Self {
        self.menu_rows = menus
            .iter()
            .map(|m| m.entries.iter().map(|e| Row { label: e.label.clone(), icon: None, hint: e.shortcut.as_ref().map(|s| s.label()), enabled: e.message.is_some(), danger: false, separator: e.separator }).collect())
            .collect();
        self.menus = menus;
        self
    }

    /// Height of the strip at the top: the title bar, or a bar just for menus.
    fn bar_height(&self) -> f32 {
        if self.chrome {
            TITLE_H
        } else if self.menus.is_empty() {
            0.0
        } else {
            MENU_H
        }
    }

    fn menu_title_at(&self, b: Rect, p: Point) -> Option<usize> {
        self.menu_titles.iter().position(|(_, r)| Rect::new(b.x + r.x, b.y + r.y, r.w, r.h).contains(p))
    }

    /// Where menu `i`'s dropdown goes: under its title, kept inside the window.
    fn dropdown(&self, b: Rect, i: usize) -> Rect {
        let size = self.menu_lists[i].size;
        let title = self.menu_titles[i].1;
        let x = (b.x + title.x).min(b.right() - size.w - 4.0).max(b.x + 4.0);
        Rect::new(x.round(), (b.y + title.y + title.h + 2.0).round(), size.w, size.h)
    }

    fn controls(&self, b: Rect) -> [Rect; 3] {
        let y = b.y + (TITLE_H - CTL) * 0.5;
        let right = b.right() - 16.0;
        [2.0, 1.0, 0.0].map(|i| Rect::new(right - CTL - i * (CTL + CTL_GAP), y, CTL, CTL))
    }
}

impl<M: Clone + 'static> Widget<M> for Frame<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.content
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let top = self.bar_height();
        if self.chrome {
            let spec = cx.theme().text(TextRole::Strong);
            self.title_layout = Some(cx.text().layout(&self.title, &spec.style(), None));
        }
        let style = cx.theme().text(TextRole::Body).style();
        let mut x = 8.0;
        self.menu_titles.clear();
        for m in &self.menus {
            let text = cx.text().layout(&m.title, &style, None);
            let w = text.size().w + 18.0;
            self.menu_titles.push((text, Rect::new(x, ((top - MENU_TITLE_H) * 0.5).round(), w, MENU_TITLE_H)));
            x += w + 2.0;
        }
        self.menu_lists = self.menu_rows.iter().map(|rows| RowList::layout(cx, rows)).collect();
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
                // With menus at the left, the title moves to the middle.
                let x = match self.menu_titles.last() {
                    Some((_, last)) => ((b.w - l.size().w) * 0.5).max(last.right() + 16.0).round(),
                    None => 20.0,
                };
                cx.scene.text(l, Point::new(b.x + x, y), p.text);
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
        let (open, row, over) = {
            let st = cx.state::<FrameState>();
            (st.menu_open, st.menu_row, st.menu_title)
        };
        if !self.menus.is_empty() {
            if !self.chrome && self.background {
                cx.scene.fill(Rect::new(b.x, b.y + MENU_H - 1.0, b.w, 1.0), 0.0, p.line, None);
            }
            for (i, (text, r)) in self.menu_titles.iter().enumerate() {
                let r = Rect::new(b.x + r.x, b.y + r.y, r.w, r.h);
                if open == Some(i) {
                    let mut paint = theme.paint(Surface::Pressed);
                    paint.shadows.clear();
                    cx.scene.paint(r, theme.small_radius(), &paint);
                } else if over == Some(i) {
                    let hover = theme.paint(Surface::Inset).fill;
                    cx.scene.fill(r, theme.small_radius(), hover.with_alpha(hover.a * 0.8), None);
                }
                cx.scene.text(text, Point::new(r.x + ((r.w - text.size().w) * 0.5).round(), r.y + ((r.h - text.size().h) * 0.5).round()), p.text);
            }
        }
        cx.with_content_color(p.text, |cx| self.content[0].draw(cx));
        if let Some(i) = open.filter(|i| *i < self.menus.len()) {
            self.menu_lists[i].draw(cx, &self.menu_rows[i], self.dropdown(b, i), row);
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        if !self.menus.is_empty() {
            let open = cx.state::<FrameState>().menu_open.filter(|i| *i < self.menus.len());
            match event {
                Event::PointerMoved { pos } => {
                    let title = self.menu_title_at(b, *pos);
                    // With a menu open, moving along the bar opens the others.
                    let now_open = match (open, title) {
                        (Some(_), Some(t)) => Some(t),
                        _ => open,
                    };
                    let row = now_open.and_then(|i| self.menu_lists[i].item_at(&self.menu_rows[i], self.dropdown(b, i), *pos));
                    if title.is_some() || row.is_some() {
                        cx.set_cursor(CursorIcon::Pointer);
                    }
                    let st = cx.state::<FrameState>();
                    if (st.menu_open, st.menu_row, st.menu_title) != (now_open, row, title) {
                        (st.menu_open, st.menu_row, st.menu_title) = (now_open, row, title);
                        cx.request_redraw();
                    }
                    if now_open.is_some() {
                        return Status::Captured;
                    }
                }
                Event::PointerLeft => cx.state::<FrameState>().menu_title = None,
                Event::PointerPressed { pos, button } => {
                    if let Some(t) = self.menu_title_at(b, *pos).filter(|_| *button == PointerButton::Primary) {
                        let st = cx.state::<FrameState>();
                        st.menu_open = if open == Some(t) { None } else { Some(t) };
                        st.menu_row = None;
                        cx.request_redraw();
                        return Status::Captured;
                    }
                    if let Some(i) = open {
                        // A press with a menu open chooses a row or closes it,
                        // and goes no further either way.
                        let rect = self.dropdown(b, i);
                        let chosen = self.menu_lists[i].item_at(&self.menu_rows[i], rect, *pos);
                        if chosen.is_some() || !rect.contains(*pos) {
                            let st = cx.state::<FrameState>();
                            st.menu_open = None;
                            st.menu_row = None;
                            cx.request_redraw();
                        }
                        if let Some(m) = chosen.and_then(|r| self.menus[i].entries[r].message.clone()) {
                            cx.emit(m);
                        }
                        return Status::Captured;
                    }
                }
                Event::Wheel { .. } | Event::WindowFocus(false) if open.is_some() => {
                    cx.state::<FrameState>().menu_open = None;
                    cx.request_redraw();
                    return Status::Captured;
                }
                Event::Key(k) if k.pressed && open.is_some() => {
                    if k.key == Key::Escape {
                        cx.state::<FrameState>().menu_open = None;
                        cx.request_redraw();
                    }
                    // Keys do not reach what is underneath while a menu is open.
                    return Status::Captured;
                }
                _ => {}
            }
        }
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
