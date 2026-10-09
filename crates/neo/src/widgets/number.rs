//! A number to change: dragged up and down like a slider with no ends,
//! nudged with the arrow keys, or typed.
//!
//! For the properties of things, where a number has no natural least and
//! most to make a slider of and is as often typed as dragged.

use armature_render::{Color, Point, Rect, Size, TextLayout};
use neo_theme::{Surface, TextRole};

use crate::core::{Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, ResizeEdge, Widget};
use crate::event::{Event, Key, PointerButton, Status};
use crate::{FocusRing, ThemeCx};

/// How a number is written: with as many places as its step has, and no
/// more noughts after the point than say anything.
pub fn written(value: f64, step: f64) -> String {
    let places = if step >= 1.0 { 0 } else { (-step.log10()).ceil().clamp(0.0, 6.0) as usize + 1 };
    let text = format!("{value:.places$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    // Nought is nought, whichever side it was come to from.
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

#[derive(Default)]
struct NumberState {
    /// Pressed here, with this value; `dragging` once moved far enough.
    pressed: Option<(Point, f64)>,
    dragging: bool,
    /// What is being typed in place of the number, and whether the first
    /// key is still to replace what was there.
    typing: Option<(String, bool)>,
    hovered: bool,
}

/// A number field: see [`number_field`].
pub struct NumberField<M> {
    value: f64,
    step: f64,
    range: (f64, f64),
    label: Option<(String, Option<Color>)>,
    unit: String,
    width: Length,
    on_change: Option<Box<dyn Fn(f64) -> M>>,
    on_scrub: Option<Box<dyn Fn(bool) -> M>>,
    shown: Option<TextLayout>,
    label_layout: Option<TextLayout>,
}

/// A field showing `value`, to be dragged, nudged or typed over.
pub fn number_field<M>(value: f64) -> NumberField<M> {
    NumberField { value, step: 0.1, range: (f64::NEG_INFINITY, f64::INFINITY), label: None, unit: String::new(), width: Length::Fixed(96.0), on_change: None, on_scrub: None, shown: None, label_layout: None }
}

const HEIGHT: f32 = 28.0;
const PAD: f32 = 8.0;
const SLACK: f32 = 3.0;

impl<M> NumberField<M> {
    /// Told the number as it changes: at every move of a drag, each nudge,
    /// and when what was typed is entered.
    pub fn on_change(mut self, f: impl Fn(f64) -> M + 'static) -> Self {
        self.on_change = Some(Box::new(f));
        self
    }

    /// Told when a drag of the number begins (true) and when it ends: all
    /// the changes between the two are one change to whoever keeps a
    /// history of them. A nudge or a number typed is one change of itself.
    pub fn on_scrub(mut self, f: impl Fn(bool) -> M + 'static) -> Self {
        self.on_scrub = Some(Box::new(f));
        self
    }

    /// How much one arrow key, or a few pixels of dragging, changes it,
    /// and so how many places it is written to. A tenth, to begin with.
    pub fn step(mut self, step: f64) -> Self {
        self.step = if step > 0.0 && step.is_finite() { step } else { 0.1 };
        self
    }

    /// The least and most it may be.
    pub fn range(mut self, range: std::ops::RangeInclusive<f64>) -> Self {
        self.range = (*range.start(), *range.end());
        self
    }

    /// A word or letter before the number, inside the field: `X`.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some((label.into(), None));
        self
    }

    /// As [`label`](Self::label), in a colour of its own: an axis's.
    pub fn tinted(mut self, label: impl Into<String>, color: Color) -> Self {
        self.label = Some((label.into(), Some(color)));
        self
    }

    /// What follows the number: `°`, ` m`.
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    fn within(&self, v: f64) -> f64 {
        // To the step's own places, so that a tenth added ten times is one.
        let v = (v / self.step * 1000.0).round() / 1000.0 * self.step;
        let v = (v * 1e9).round() / 1e9;
        v.clamp(self.range.0, self.range.1)
    }

    fn say(&self, cx: &mut EventCx<M>, v: f64) {
        let v = self.within(v);
        if let (Some(f), true) = (&self.on_change, v != self.value && v.is_finite()) {
            cx.emit(f(v));
        }
    }
}

impl<M: 'static> Widget<M> for NumberField<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn focusable(&self) -> bool {
        true
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let style = cx.theme().text(TextRole::Body).style();
        let typing = cx.state::<NumberState>().typing.as_ref().map(|(t, _)| t.clone());
        let text = typing.unwrap_or_else(|| format!("{}{}", written(self.value, self.step), self.unit));
        self.shown = Some(cx.text().layout(&text, &style, None));
        let label = cx.theme().text(TextRole::Caption).style();
        self.label_layout = self.label.as_ref().map(|(l, _)| cx.text().layout(l, &label, None));
        limits.constrain(self.width, Length::Shrink).resolve(Size::new(96.0, HEIGHT))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let (typing, dragging, hovered) = {
            let st = cx.state::<NumberState>();
            (st.typing.clone(), st.dragging, st.hovered)
        };
        cx.scene.paint(b, theme.small_radius(), &theme.paint(if dragging { Surface::Pressed } else if hovered || typing.is_some() { Surface::Hovered } else { Surface::Inset }));
        cx.focus_ring(b, theme.small_radius());
        let mut x = b.x + PAD;
        if let (Some(layout), Some((_, color))) = (&self.label_layout, &self.label) {
            let s = layout.size();
            cx.scene.text(layout, Point::new(x, b.y + ((b.h - s.h) * 0.5).round()), color.unwrap_or(p.muted));
            x += s.w + 6.0;
        }
        let Some(shown) = &self.shown else { return };
        let s = shown.size();
        // The number to the right, as figures in a column are; what is typed, from the left.
        let at = if typing.is_some() { x } else { (b.right() - PAD - s.w).max(x) };
        cx.scene.push_clip(Rect::new(x, b.y, (b.right() - PAD - x).max(0.0), b.h));
        let y = b.y + ((b.h - s.h) * 0.5).round();
        if let Some((_, fresh)) = &typing {
            // All of it marked while the first key would replace it; then a caret after it.
            if *fresh {
                cx.scene.fill(Rect::new(at - 1.0, y, s.w + 2.0, s.h), 2.0, p.accent.with_alpha(0.3), None);
            } else {
                cx.scene.fill(Rect::new(at + s.w + 1.0, y + 1.0, 1.5, s.h - 2.0), 0.0, p.text, None);
            }
        }
        cx.scene.text(shown, Point::new(at, y), p.text);
        cx.scene.pop_clip();
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        // Typing ends when the keyboard goes elsewhere: what was typed is the number.
        if cx.state::<NumberState>().typing.is_some() && !cx.is_focused() && !matches!(event, Event::WindowFocus(_)) {
            let (typed, _) = cx.state::<NumberState>().typing.take().expect("checked");
            if let Ok(v) = typed.trim().parse::<f64>() {
                self.say(cx, v);
            }
            cx.request_layout();
        }
        match event {
            Event::PointerMoved { pos } => {
                let st = cx.state::<NumberState>();
                if let Some((from, was)) = st.pressed {
                    let moved = pos.x - from.x;
                    let began = !st.dragging && moved.abs() > SLACK;
                    st.dragging |= began;
                    if let (true, Some(f)) = (began, &self.on_scrub) {
                        cx.emit(f(true));
                    }
                    if cx.state::<NumberState>().dragging {
                        cx.set_cursor(CursorIcon::Resize(ResizeEdge::East));
                        // A step for every few pixels; finer with Shift held, coarser with Control.
                        let m = cx.modifiers();
                        let per = self.step * if m.shift { 0.1 } else if m.ctrl { 10.0 } else { 1.0 };
                        self.say(cx, was + f64::from((moved / 4.0).round()) * per);
                    }
                    return Status::Captured;
                }
                let over = b.contains(*pos);
                if over {
                    cx.set_cursor(CursorIcon::Resize(ResizeEdge::East));
                }
                if cx.state::<NumberState>().hovered != over {
                    cx.state::<NumberState>().hovered = over;
                    cx.request_redraw();
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                cx.state::<NumberState>().hovered = false;
                Status::Ignored
            }
            // A press anywhere else ends the typing, and what was typed is the number.
            Event::PointerPressed { pos, .. } if !b.contains(*pos) && cx.state::<NumberState>().typing.is_some() => {
                let (typed, _) = cx.state::<NumberState>().typing.take().expect("checked");
                if let Ok(v) = typed.trim().parse::<f64>() {
                    self.say(cx, v);
                }
                cx.release_focus();
                cx.request_layout();
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.request_focus();
                let st = cx.state::<NumberState>();
                (st.pressed, st.dragging) = (Some((*pos, self.value)), false);
                Status::Captured
            }
            // Let go somewhere else, on another control say, with a number half typed: it is entered.
            Event::PointerReleased { pos, .. } if !b.contains(*pos) && cx.state::<NumberState>().pressed.is_none() && cx.state::<NumberState>().typing.is_some() => {
                let (typed, _) = cx.state::<NumberState>().typing.take().expect("checked");
                if let Ok(v) = typed.trim().parse::<f64>() {
                    self.say(cx, v);
                }
                cx.request_layout();
                Status::Ignored
            }
            Event::PointerReleased { button: PointerButton::Primary, .. } => {
                let st = cx.state::<NumberState>();
                let (Some(_), dragged) = (st.pressed.take(), std::mem::take(&mut st.dragging)) else { return Status::Ignored };
                // Only clicked: the number is to be typed over.
                if !dragged && st.typing.is_none() {
                    st.typing = Some((written(self.value, self.step), true));
                }
                if let (true, Some(f)) = (dragged, &self.on_scrub) {
                    cx.emit(f(false));
                }
                cx.request_layout();
                Status::Captured
            }
            Event::Key(k) if k.pressed && cx.is_focused() => {
                let typing = cx.state::<NumberState>().typing.clone();
                match (&k.key, typing) {
                    (Key::Enter, Some((typed, _))) => {
                        cx.state::<NumberState>().typing = None;
                        if let Ok(v) = typed.trim().parse::<f64>() {
                            self.say(cx, v);
                        }
                    }
                    (Key::Escape, Some(_)) => cx.state::<NumberState>().typing = None,
                    (Key::Backspace, Some((mut typed, fresh))) => {
                        if fresh {
                            typed.clear();
                        } else {
                            typed.pop();
                        }
                        cx.state::<NumberState>().typing = Some((typed, false));
                    }
                    (Key::Enter, None) => cx.state::<NumberState>().typing = Some((written(self.value, self.step), true)),
                    (Key::Up, None) => self.say(cx, self.value + self.step),
                    (Key::Down, None) => self.say(cx, self.value - self.step),
                    (_, typing) => {
                        // Figures, a point, a sign, an exponent: what a number is written with.
                        let Some(more) = k.text.as_deref().filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))) else { return Status::Ignored };
                        if cx.modifiers().logo || cx.modifiers().ctrl {
                            return Status::Ignored;
                        }
                        let typed = match typing {
                            Some((typed, false)) => typed + more,
                            // The first key takes the place of what was there.
                            _ => more.to_owned(),
                        };
                        cx.state::<NumberState>().typing = Some((typed, false));
                    }
                }
                cx.request_layout();
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

impl<M: 'static> From<NumberField<M>> for Element<M> {
    fn from(w: NumberField<M>) -> Self {
        Element::new(w)
    }
}

/// The colours the three axes go by, wherever axes are drawn: red for X,
/// green for Y and blue for Z, and grey for a fourth.
pub const AXES: [(&str, u32); 4] = [("X", 0xE5484D), ("Y", 0x46A758), ("Z", 0x3E63DD), ("W", 0x8B8D98)];

/// A field to each part of a vector, side by side: two, three or four of
/// them, named and coloured as axes. `on_change` is told which part was
/// changed, and to what.
pub fn vector_field<M: 'static>(parts: &[f64], step: f64, on_change: impl Fn(usize, f64) -> M + Clone + 'static) -> Element<M> {
    vector_fields(parts, step, on_change, None::<fn(bool) -> M>)
}

/// As [`vector_field`], and told when a drag of any part begins and ends:
/// see [`NumberField::on_scrub`].
pub fn vector_field_scrubbed<M: 'static>(parts: &[f64], step: f64, on_change: impl Fn(usize, f64) -> M + Clone + 'static, on_scrub: impl Fn(bool) -> M + Clone + 'static) -> Element<M> {
    vector_fields(parts, step, on_change, Some(on_scrub))
}

fn vector_fields<M: 'static>(parts: &[f64], step: f64, on_change: impl Fn(usize, f64) -> M + Clone + 'static, on_scrub: Option<impl Fn(bool) -> M + Clone + 'static>) -> Element<M> {
    let mut fields = crate::widgets::row().spacing(4.0).width(Length::Fill);
    for (i, value) in parts.iter().enumerate().take(AXES.len()) {
        let say = on_change.clone();
        let (name, color) = AXES[i];
        let mut field = number_field(*value).step(step).tinted(name, Color::hex(color)).width(Length::Fill).on_change(move |v| say(i, v));
        if let Some(scrub) = on_scrub.clone() {
            field = field.on_scrub(scrub);
        }
        fields = fields.push(field);
    }
    fields.into()
}

/// A field that stands for something else: an entity, a file, a
/// material. It shows the name it is given, or that there is none; a
/// click on it asks for another to be chosen, which is the app's to
/// offer, and the cross beside it, where there is one, clears it.
pub fn reference_field<M: Clone + 'static>(glyph: neo_theme::Icon, name: Option<&str>, on_pick: M, on_clear: Option<M>) -> Element<M> {
    use crate::widgets::{icon, icon_button, row, text, Button, ButtonKind, Tone};
    let shown = row().spacing(8.0).align(crate::core::Align::Center).width(Length::Fill).push(icon(glyph).size(15.0).tone(Tone::Muted)).push(match name {
        Some(name) => text(name.to_owned()).width(Length::Fill),
        None => text("None").tone(Tone::Faint).width(Length::Fill),
    });
    let mut field = row().spacing(4.0).align(crate::core::Align::Center).width(Length::Fill).push(Button::new(shown).kind(ButtonKind::Ghost).width(Length::Fill).on_press(on_pick));
    if let (Some(clear), Some(_)) = (on_clear, name) {
        field = field.push(icon_button(neo_theme::icons::X, 28.0).kind(ButtonKind::Ghost).on_press(clear));
    }
    field.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_written_to_the_places_its_step_has() {
        assert_eq!((written(3.0, 1.0), written(3.4, 1.0), written(-12.0, 5.0)), ("3".into(), "3".into(), "-12".into()));
        assert_eq!((written(0.5, 0.1), written(1.0, 0.1), written(0.25, 0.1), written(-0.04, 0.1)), ("0.5".into(), "1".into(), "0.25".into(), "-0.04".into()));
        assert_eq!((written(1.23456, 0.01), written(1.2, 0.001), written(0.000001, 0.001)), ("1.235".into(), "1.2".into(), "0".into()));
        assert_eq!((written(-0.0, 0.1), written(-0.0001, 0.1)), ("0".into(), "0".into()), "nought has no sign");
        assert_eq!(written(1e9, 0.1), "1000000000");
    }

    #[test]
    fn a_number_is_kept_to_its_step_and_within_its_range() {
        let field = number_field::<()>(0.0).step(0.1).range(-1.0..=1.0);
        // A tenth and two tenths are three tenths, not three and a little.
        assert_eq!(field.within(0.1 + 0.2), 0.3);
        assert_eq!((field.within(7.0), field.within(-7.0), field.within(0.04), field.within(0.06)), (1.0, -1.0, 0.04, 0.06));
        assert_eq!(number_field::<()>(0.0).step(-3.0).step, 0.1, "a step that is none keeps the usual one");
    }
}
