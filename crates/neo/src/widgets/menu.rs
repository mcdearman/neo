use armature_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Icon, Shadow, Surface, TextRole};

use crate::ThemeCx;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, Status};

const ITEM_H: f32 = 32.0;
const SEPARATOR_H: f32 = 9.0;
const PAD: f32 = 6.0;
const ICON: f32 = 15.0;

/// One row of a [`PopupMenu`].
pub struct MenuItem<M> {
    label: String,
    icon: Option<Icon>,
    message: Option<M>,
    danger: bool,
    separator: bool,
}

impl<M> MenuItem<M> {
    /// A row that sends `message` when chosen.
    pub fn new(label: impl Into<String>, message: M) -> Self {
        Self { label: label.into(), icon: None, message: Some(message), danger: false, separator: false }
    }

    /// A greyed-out row that cannot be chosen.
    pub fn disabled(label: impl Into<String>) -> Self {
        Self { label: label.into(), icon: None, message: None, danger: false, separator: false }
    }

    /// A thin line between groups of rows.
    pub fn separator() -> Self {
        Self { label: String::new(), icon: None, message: None, danger: false, separator: true }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Marks a destructive choice, such as deleting.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

#[derive(Default)]
struct MenuState {
    hovered: Option<usize>,
}

/// A menu that pops up at a point, such as a context menu. Put it last in
/// a [`Stack`](super::Stack) over the window's content: it fills the stack
/// so that a click anywhere else dismisses it.
pub struct PopupMenu<M> {
    at: Point,
    items: Vec<MenuItem<M>>,
    on_dismiss: M,
    layouts: Vec<Option<(TextLayout, Option<TextLayout>)>>,
    size: Size,
}

impl<M: Clone> PopupMenu<M> {
    /// `at` is where the menu's top-left corner goes, in window
    /// coordinates. It is moved as needed to stay inside the window.
    pub fn new(at: Point, items: Vec<MenuItem<M>>, on_dismiss: M) -> Self {
        Self { at, items, on_dismiss, layouts: vec![], size: Size::ZERO }
    }

    /// The menu's rectangle in window coordinates, moved as needed to stay
    /// inside `bounds`, the area this widget covers.
    fn rect(&self, bounds: Rect) -> Rect {
        let x = self.at.x.min(bounds.right() - self.size.w - 4.0).max(bounds.x + 4.0);
        let y = self.at.y.min(bounds.bottom() - self.size.h - 4.0).max(bounds.y + 4.0);
        Rect::new(x.round(), y.round(), self.size.w, self.size.h)
    }

    fn item_at(&self, bounds: Rect, p: Point) -> Option<usize> {
        let menu = self.rect(bounds);
        if !menu.contains(p) {
            return None;
        }
        let mut y = menu.y + PAD;
        for (i, item) in self.items.iter().enumerate() {
            let h = if item.separator { SEPARATOR_H } else { ITEM_H };
            if p.y >= y && p.y < y + h {
                return (!item.separator && item.message.is_some()).then_some(i);
            }
            y += h;
        }
        None
    }
}

/// Shorthand for [`PopupMenu::new`].
pub fn popup_menu<M: Clone>(at: Point, items: Vec<MenuItem<M>>, on_dismiss: M) -> PopupMenu<M> {
    PopupMenu::new(at, items, on_dismiss)
}

impl<M: Clone + 'static> Widget<M> for PopupMenu<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = limits.max;
        let style = cx.theme().text(TextRole::Body).style();
        let icon_style = TextStyle { size: ICON, weight: 400, family: FontFamily::Icons, line_height: 1.0, letter_spacing: 0.0 };
        let any_icon = self.items.iter().any(|i| i.icon.is_some());
        let mut width: f32 = 160.0;
        let mut height = PAD * 2.0;
        self.layouts.clear();
        for item in &self.items {
            if item.separator {
                self.layouts.push(None);
                height += SEPARATOR_H;
                continue;
            }
            let label = cx.text().layout(&item.label, &style, None);
            let glyph = item.icon.map(|g| cx.text().layout(&g.0.to_string(), &icon_style, None));
            width = width.max(label.size().w + 28.0 + if any_icon { ICON + 10.0 } else { 0.0 });
            self.layouts.push(Some((label, glyph)));
            height += ITEM_H;
        }
        self.size = Size::new(width + PAD * 2.0, height);
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let theme = *cx.theme();
        let p = theme.palette();
        let menu = self.rect(cx.bounds());
        let hovered = cx.state::<MenuState>().hovered;
        let any_icon = self.items.iter().any(|i| i.icon.is_some());

        // Menus sit over everything, whatever the glass setting.
        cx.scene.push_layer();
        let radius = theme.small_radius() + 2.0;
        cx.scene.shadow(menu, radius, &Shadow { offset: (0.0, 6.0), blur: 24.0, spread: 0.0, color: neo_theme::Color::BLACK.with_alpha(0.22), inset: false });
        cx.scene.fill(menu, radius, p.surface, Some((1.0, p.line)));

        let mut y = menu.y + PAD;
        for (i, (item, layout)) in self.items.iter().zip(&self.layouts).enumerate() {
            let Some((label, glyph)) = layout else {
                cx.scene.fill(Rect::new(menu.x + PAD, (y + SEPARATOR_H * 0.5).floor(), menu.w - PAD * 2.0, 1.0), 0.0, p.line, None);
                y += SEPARATOR_H;
                continue;
            };
            let row = Rect::new(menu.x + PAD, y, menu.w - PAD * 2.0, ITEM_H);
            let enabled = item.message.is_some();
            let color = if !enabled {
                p.faint
            } else if item.danger {
                p.bad
            } else {
                p.text
            };
            if hovered == Some(i) {
                let mut paint = theme.paint(Surface::Pressed);
                paint.shadows.clear();
                cx.scene.paint(row, theme.small_radius(), &paint);
            }
            let mut x = row.x + 10.0;
            if any_icon {
                if let Some(g) = glyph {
                    cx.scene.text(g, Point::new(x, row.y + ((ITEM_H - g.size().h) * 0.5).round()), if item.danger && enabled { p.bad } else { p.muted });
                }
                x += ICON + 10.0;
            }
            cx.scene.text(label, Point::new(x, row.y + ((ITEM_H - label.size().h) * 0.5).round()), color);
            y += ITEM_H;
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let bounds = cx.bounds();
        match event {
            Event::PointerMoved { pos } => {
                let h = self.item_at(bounds, *pos);
                if h.is_some() {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                let st = cx.state::<MenuState>();
                if st.hovered != h {
                    st.hovered = h;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            // Any press belongs to the menu: on a row it chooses, on a
            // separator or disabled row it does nothing, elsewhere it dismisses.
            Event::PointerPressed { pos, .. } => {
                let inside = self.rect(bounds).contains(*pos);
                match self.item_at(bounds, *pos).and_then(|i| self.items[i].message.clone()) {
                    Some(m) => cx.emit(m),
                    None if !inside => cx.emit(self.on_dismiss.clone()),
                    None => {}
                }
                Status::Captured
            }
            Event::Wheel { .. } => {
                cx.emit(self.on_dismiss.clone());
                Status::Captured
            }
            Event::Key(k) if k.pressed => {
                if k.key == Key::Escape {
                    cx.emit(self.on_dismiss.clone());
                }
                // Keys do not reach what is underneath while a menu is open.
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
