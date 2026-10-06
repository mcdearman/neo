//! Building blocks the desktop apps share, so they look like one family.

use neo::prelude::*;
use neo::theme::Icon;
use neo::theme::Shadow;
use neo::{Cx, DrawCx, Event, EventCx, Key, Limits, Point, Size, Status, ThemeCx, Widget};

use crate::{Desktop, DesktopMsg};

/// Width of an app's navigation sidebar.
pub const SIDEBAR_W: f32 = 212.0;

/// A navigation entry: icon and label, highlighted when selected.
pub fn nav_item<M: Clone + 'static>(glyph: Icon, label: impl Into<String>, selected: bool, on_press: M) -> Element<M> {
    Button::new(row().spacing(10.0).align(Align::Center).push(icon(glyph).size(16.0)).push(text(label).role(TextRole::Strong).no_wrap()))
        .kind(ButtonKind::Ghost)
        .selected(selected)
        .on_press(on_press)
        .width(Length::Fill)
        .padding([10.0, 8.0])
        .align_x(Align::Start)
        .into()
}

/// A small uppercase heading over a group of controls or nav items.
pub fn section<M: 'static>(label: &str) -> Element<M> {
    container(text(label).role(TextRole::Label).tone(Tone::Muted)).padding([10.0, 10.0, 4.0, 10.0]).into()
}

/// A settings row: title and help text on the left, the control on the right.
pub fn setting<M: Clone + 'static>(title: &str, help: &str, control: impl Into<Element<M>>) -> Element<M> {
    row()
        .spacing(16.0)
        .align(Align::Center)
        .width(Length::Fill)
        .push(column().spacing(2.0).width(Length::Fill).push(text(title).role(TextRole::Strong)).push(text(help).role(TextRole::Caption).tone(Tone::Muted)))
        .push(control)
        .into()
}

/// The sidebar column that sits flush against the window's left edge.
pub fn sidebar<M: 'static>(content: impl Into<Element<M>>) -> Element<M> {
    container(content).padding([6.0, 10.0, 12.0, 12.0]).width(SIDEBAR_W).height(Length::Fill).into()
}

/// The main pane beside a sidebar: a card that fills the rest of the window.
pub fn pane<M: 'static>(content: impl Into<Element<M>>) -> Element<M> {
    container(content).surface(Surface::Card).width(Length::Fill).height(Length::Fill).into()
}

/// The usual layout: sidebar on the left, card pane on the right.
pub fn split<M: 'static>(side: impl Into<Element<M>>, main: impl Into<Element<M>>) -> Element<M> {
    row().spacing(0.0).width(Length::Fill).height(Length::Fill).padding([0.0, 12.0, 12.0, 0.0]).push(sidebar(side)).push(pane(main)).into()
}

/// A one-line notice, such as an error from the last action.
pub fn notice<M: 'static>(tone: Tone, message: impl Into<String>) -> Element<M> {
    let glyph = match tone {
        Tone::Bad | Tone::Warn => neo::icons::TRIANGLE_ALERT,
        _ => neo::icons::CIRCLE_CHECK,
    };
    row().spacing(6.0).align(Align::Center).push(icon(glyph).size(14.0).tone(tone)).push(text(message).role(TextRole::Caption).tone(tone)).into()
}

/// Dims what is behind a panel and keeps the pointer and keyboard from
/// reaching it. A click on it, or Escape, sends `on_dismiss`.
struct Scrim<M> {
    on_dismiss: M,
}

impl<M: Clone + 'static> Widget<M> for Scrim<M> {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        // A new layer, so the dimming and the panel go over the text
        // behind them as well as the shapes.
        cx.scene.push_layer();
        cx.scene.fill(b, 0.0, Color::BLACK.with_alpha(0.3), None);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        match event {
            Event::PointerPressed { .. } => cx.emit(self.on_dismiss.clone()),
            Event::Key(k) if k.pressed && k.key == Key::Escape => cx.emit(self.on_dismiss.clone()),
            // Tab still moves between the panel's controls.
            Event::Key(k) if k.key == Key::Tab => return Status::Ignored,
            Event::PointerMoved { .. } | Event::PointerReleased { .. } | Event::Wheel { .. } | Event::Key(_) => {}
            _ => return Status::Ignored,
        }
        Status::Captured
    }
}

/// A solid sheet for a panel's content. Unlike a card it is never
/// see-through, so it stays readable over anything, glass window or not,
/// and a click on its empty parts stays with it.
struct Sheet<M> {
    content: [Element<M>; 1],
    width: f32,
}

const SHEET_PAD: f32 = 20.0;

impl<M: 'static> Widget<M> for Sheet<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.content
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let inner = (self.width.min(limits.max.w) - SHEET_PAD * 2.0).max(0.0);
        let size = self.content[0].layout(cx, Limits::new(Size::new(inner, 0.0), Size::new(inner, f32::INFINITY)));
        self.content[0].set_position(Point::new(SHEET_PAD, SHEET_PAD));
        Size::new(inner + SHEET_PAD * 2.0, size.h + SHEET_PAD * 2.0)
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let radius = theme.control_radius() + 2.0;
        cx.scene.shadow(b, radius, &Shadow { offset: (0.0, 10.0), blur: 36.0, spread: 0.0, color: Color::BLACK.with_alpha(0.28), inset: false });
        cx.scene.fill(b, radius, p.surface, Some((1.0, p.line)));
        cx.with_content_color(p.text, |cx| self.content[0].draw(cx));
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let status = self.content[0].event(cx, event);
        match event {
            Event::PointerPressed { pos, .. } if cx.bounds().contains(*pos) => Status::Captured,
            _ => status,
        }
    }
}

/// A solid sheet `width` wide holding `content`, with a shadow. It stays
/// readable over anything, which a card on a glass window does not.
pub fn sheet<M: 'static>(content: impl Into<Element<M>>, width: f32) -> Element<M> {
    Element::new(Sheet { content: [content.into()], width })
}

/// An app's settings panel, shown over `view`: the app's own rows, then
/// the ones every Neo app has.
pub(crate) fn settings_panel<M: Clone + 'static>(desktop: &Desktop, view: Element<M>, heading: &str, wrap: impl Fn(DesktopMsg) -> M + Clone + 'static, extra: Vec<Element<M>>) -> Element<M> {
    let close = wrap(DesktopMsg::CloseSettings);
    let help = if desktop.appearance.glass.enabled {
        "Translucent, with what is behind the window blurred. Turn off to make this app's window solid."
    } else {
        "Glass windows are turned off in Settings, so this has no effect for now."
    };
    let glass = wrap.clone();
    let mut rows = column()
        .spacing(16.0)
        .width(Length::Fill)
        .push(row().align(Align::Center).push(text(heading.to_owned()).role(TextRole::Title)).push(Space::fill_x()).push(icon_button(neo::icons::X, 28.0).kind(ButtonKind::Ghost).on_press(close.clone())))
        .push(Divider::horizontal());
    for e in extra {
        rows = rows.push(e);
    }
    rows = rows.push(setting("Glass window", help, toggle(desktop.prefs.glass, move |on| glass(DesktopMsg::Glass(on)))));
    if let Some(e) = &desktop.error {
        rows = rows.push(notice(Tone::Bad, e.clone()));
    }
    let panel = Element::new(Sheet { content: [rows.into()], width: 480.0 });
    stack().push(view).push(Element::new(Scrim { on_dismiss: close })).push(container(panel).width(Length::Fill).height(Length::Fill).center()).into()
}
