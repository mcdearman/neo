//! Building blocks the desktop apps share, so they look like one family.

use neo::prelude::*;
use neo::theme::Icon;

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
