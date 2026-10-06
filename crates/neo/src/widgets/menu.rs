use armature_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Icon, Shadow, Surface, TextRole};

use crate::ThemeCx;
use crate::core::{Cx, CursorIcon, DrawCx, EventCx, Length, Limits, Widget};
use crate::event::{Event, Key, Status};

pub(crate) const ITEM_H: f32 = 32.0;
const SEPARATOR_H: f32 = 9.0;
pub(crate) const PAD: f32 = 6.0;
const ICON: f32 = 15.0;

/// One row of a dropdown: what [`RowList`] measures and paints.
pub(crate) struct Row {
    pub label: String,
    pub icon: Option<Icon>,
    /// Shown at the right in a quieter colour, such as a shortcut.
    pub hint: Option<String>,
    pub enabled: bool,
    pub danger: bool,
    pub separator: bool,
}

struct RowText {
    label: TextLayout,
    glyph: Option<TextLayout>,
    hint: Option<TextLayout>,
}

/// The measured rows of a dropdown, shared by [`PopupMenu`] and the menu
/// bar so both look the same.
#[derive(Default)]
pub(crate) struct RowList {
    texts: Vec<Option<RowText>>,
    any_icon: bool,
    pub size: Size,
}

impl RowList {
    pub fn layout(cx: &mut Cx, rows: &[Row]) -> Self {
        let style = cx.theme().text(TextRole::Body).style();
        let icon_style = TextStyle { size: ICON, weight: 400, family: FontFamily::Icons, line_height: 1.0, letter_spacing: 0.0 };
        let any_icon = rows.iter().any(|r| r.icon.is_some());
        let mut width: f32 = 160.0;
        let mut height = PAD * 2.0;
        let mut texts = Vec::with_capacity(rows.len());
        for row in rows {
            if row.separator {
                texts.push(None);
                height += SEPARATOR_H;
                continue;
            }
            let label = cx.text().layout(&row.label, &style, None);
            let glyph = row.icon.map(|g| cx.text().layout(&g.0.to_string(), &icon_style, None));
            let hint = row.hint.as_ref().map(|h| cx.text().layout(h, &style, None));
            let hint_w = hint.as_ref().map_or(0.0, |h| h.size().w + 28.0);
            width = width.max(label.size().w + 28.0 + if any_icon { ICON + 10.0 } else { 0.0 } + hint_w);
            texts.push(Some(RowText { label, glyph, hint }));
            height += ITEM_H;
        }
        Self { texts, any_icon, size: Size::new(width + PAD * 2.0, height) }
    }

    /// The row under `p` that can be chosen, for a list drawn at `menu`.
    pub fn item_at(&self, rows: &[Row], menu: Rect, p: Point) -> Option<usize> {
        if !menu.contains(p) {
            return None;
        }
        let mut y = menu.y + PAD;
        for (i, row) in rows.iter().enumerate() {
            let h = if row.separator { SEPARATOR_H } else { ITEM_H };
            if p.y >= y && p.y < y + h {
                return (!row.separator && row.enabled).then_some(i);
            }
            y += h;
        }
        None
    }

    pub fn draw(&self, cx: &mut DrawCx, rows: &[Row], menu: Rect, hovered: Option<usize>) {
        let theme = *cx.theme();
        let p = theme.palette();
        // Menus sit over everything, whatever the glass setting.
        cx.scene.push_layer();
        let radius = theme.small_radius() + 2.0;
        cx.scene.shadow(menu, radius, &Shadow { offset: (0.0, 6.0), blur: 24.0, spread: 0.0, color: neo_theme::Color::BLACK.with_alpha(0.22), inset: false });
        cx.scene.fill(menu, radius, p.surface, Some((1.0, p.line)));

        let mut y = menu.y + PAD;
        for (i, (row, text)) in rows.iter().zip(&self.texts).enumerate() {
            let Some(text) = text else {
                cx.scene.fill(Rect::new(menu.x + PAD, (y + SEPARATOR_H * 0.5).floor(), menu.w - PAD * 2.0, 1.0), 0.0, p.line, None);
                y += SEPARATOR_H;
                continue;
            };
            let area = Rect::new(menu.x + PAD, y, menu.w - PAD * 2.0, ITEM_H);
            let color = if !row.enabled {
                p.faint
            } else if row.danger {
                p.bad
            } else {
                p.text
            };
            if hovered == Some(i) {
                let mut paint = theme.paint(Surface::Pressed);
                paint.shadows.clear();
                cx.scene.paint(area, theme.small_radius(), &paint);
            }
            let mut x = area.x + 10.0;
            if self.any_icon {
                if let Some(g) = &text.glyph {
                    cx.scene.text(g, Point::new(x, area.y + ((ITEM_H - g.size().h) * 0.5).round()), if row.danger && row.enabled { p.bad } else { p.muted });
                }
                x += ICON + 10.0;
            }
            cx.scene.text(&text.label, Point::new(x, area.y + ((ITEM_H - text.label.size().h) * 0.5).round()), color);
            if let Some(h) = &text.hint {
                cx.scene.text(h, Point::new(area.right() - 10.0 - h.size().w, area.y + ((ITEM_H - h.size().h) * 0.5).round()), if row.enabled { p.muted } else { p.faint });
            }
            y += ITEM_H;
        }
    }
}

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
    rows: Vec<Row>,
    list: RowList,
}

impl<M: Clone> PopupMenu<M> {
    /// `at` is where the menu's top-left corner goes, in window
    /// coordinates. It is moved as needed to stay inside the window.
    pub fn new(at: Point, items: Vec<MenuItem<M>>, on_dismiss: M) -> Self {
        let rows = items.iter().map(|i| Row { label: i.label.clone(), icon: i.icon, hint: None, enabled: i.message.is_some(), danger: i.danger, separator: i.separator }).collect();
        Self { at, items, on_dismiss, rows, list: RowList::default() }
    }

    /// The menu's rectangle in window coordinates, moved as needed to stay
    /// inside `bounds`, the area this widget covers.
    fn rect(&self, bounds: Rect) -> Rect {
        let size = self.list.size;
        let x = self.at.x.min(bounds.right() - size.w - 4.0).max(bounds.x + 4.0);
        let y = self.at.y.min(bounds.bottom() - size.h - 4.0).max(bounds.y + 4.0);
        Rect::new(x.round(), y.round(), size.w, size.h)
    }

    fn item_at(&self, bounds: Rect, p: Point) -> Option<usize> {
        self.list.item_at(&self.rows, self.rect(bounds), p)
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
        self.list = RowList::layout(cx, &self.rows);
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let menu = self.rect(cx.bounds());
        let hovered = cx.state::<MenuState>().hovered;
        self.list.draw(cx, &self.rows, menu, hovered);
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
