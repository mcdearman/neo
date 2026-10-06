use armature_render::{Point, Size};
use neo_theme::{Color, Surface};

use super::style::lerp_paint;
use crate::{FocusRing, ThemeCx};
use crate::anim::Anim;
use crate::core::{Align, Cx, CursorIcon, DrawCx, Element, EventCx, Length, Limits, Padding, Widget};
use crate::event::{Event, Key, PointerButton, Status};

/// Visual weight of a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// A standard control that lifts off the surface.
    #[default]
    Raised,
    /// The primary action, filled with the accent colour.
    Accent,
    /// No surface until hovered. For toolbars and lists.
    Ghost,
}

#[derive(Default)]
struct ButtonState {
    hovered: bool,
    pressed: bool,
    hover: Anim,
    press: Anim,
}

/// A pressable control. Pressed and selected buttons look pushed in.
pub struct Button<M> {
    content: [Element<M>; 1],
    on_press: Option<M>,
    /// Builds the message from the modifier keys held at the click.
    on_press_with: Option<Box<dyn Fn(crate::event::Modifiers) -> M>>,
    kind: ButtonKind,
    selected: bool,
    padding: Padding,
    width: Length,
    height: Length,
    radius: Option<f32>,
    round: bool,
    align: Align,
}

impl<M: Clone + 'static> Button<M> {
    pub fn new(content: impl Into<Element<M>>) -> Self {
        Self {
            content: [content.into()],
            on_press: None,
            on_press_with: None,
            kind: ButtonKind::Raised,
            selected: false,
            padding: Padding::xy(16.0, 9.0),
            width: Length::Shrink,
            height: Length::Shrink,
            radius: None,
            round: false,
            align: Align::Center,
        }
    }

    /// The message sent when the button is activated. Without one the
    /// button is disabled.
    pub fn on_press(mut self, m: M) -> Self {
        self.on_press = Some(m);
        self
    }

    pub fn on_press_maybe(mut self, m: Option<M>) -> Self {
        self.on_press = m;
        self
    }

    /// Like [`on_press`](Self::on_press), but the message can depend on the
    /// modifier keys held, as when Shift-clicking extends a selection.
    pub fn on_press_with(mut self, f: impl Fn(crate::event::Modifiers) -> M + 'static) -> Self {
        self.on_press_with = Some(Box::new(f));
        self
    }

    fn enabled(&self) -> bool {
        self.on_press.is_some() || self.on_press_with.is_some()
    }

    fn message(&self, modifiers: crate::event::Modifiers) -> Option<M> {
        match &self.on_press_with {
            Some(f) => Some(f(modifiers)),
            None => self.on_press.clone(),
        }
    }

    pub fn kind(mut self, k: ButtonKind) -> Self {
        self.kind = k;
        self
    }

    /// Draws the button as pressed in, for toggles and current items.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }

    pub fn padding(mut self, p: impl Into<Padding>) -> Self {
        self.padding = p.into();
        self
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }

    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }

    /// Horizontal placement of the content when the button is wider than it.
    pub fn align_x(mut self, a: Align) -> Self {
        self.align = a;
        self
    }

    /// A circular button sized to its content.
    pub fn round(mut self) -> Self {
        self.round = true;
        self
    }
}

/// A raised button with a text label.
pub fn button<M: Clone + 'static>(label: impl Into<String>) -> Button<M> {
    Button::new(Element::new(super::Text::new(label).role(neo_theme::TextRole::Strong)))
}

/// A round button holding an icon.
pub fn icon_button<M: Clone + 'static>(icon: neo_theme::Icon, size: f32) -> Button<M> {
    let pad = (size * 0.25).round();
    Button::new(Element::new(super::Icon::new(icon).size(size - pad * 2.0))).padding(pad).round()
}

impl<M: Clone + 'static> Widget<M> for Button<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn height(&self) -> Length {
        self.height
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.content
    }

    fn focusable(&self) -> bool {
        self.enabled()
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let limits = limits.constrain(self.width, self.height);
        let inner = limits.shrink(self.padding);
        let cs = self.content[0].layout(cx, Limits::loose(inner.max));
        let mut size = limits.resolve(Size::new(cs.w + self.padding.horizontal(), cs.h + self.padding.vertical()));
        if self.round {
            let d = size.w.max(size.h);
            size = Size::new(d, d);
        }
        self.content[0].set_position(Point::new(if self.align == Align::Center { ((size.w - cs.w) * 0.5).round() } else { self.padding.left + self.align.offset(size.w - self.padding.horizontal() - cs.w) }, ((size.h - cs.h) * 0.5).round()));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let now = cx.now();
        let motion = theme.motion();
        let enabled = self.enabled();
        let (hover_t, press_t, animating) = {
            let st = cx.state::<ButtonState>();
            let h = st.hover.step(if st.hovered && enabled { 1.0 } else { 0.0 }, now, motion);
            let pr = st.press.step(if (st.pressed && enabled) || self.selected { 1.0 } else { 0.0 }, now, motion * 0.7);
            (h, pr, st.hover.is_animating() || st.press.is_animating())
        };
        if animating {
            cx.request_animation();
        }
        let radius = if self.round { b.w.min(b.h) * 0.5 } else { self.radius.unwrap_or(theme.control_radius()) };

        let mut paint = match self.kind {
            ButtonKind::Raised => {
                let rest = lerp_paint(&theme.paint(Surface::Raised), &theme.paint(Surface::Hovered), hover_t);
                lerp_paint(&rest, &theme.paint(Surface::Pressed), press_t)
            }
            ButtonKind::Accent => {
                let mut a = theme.paint(Surface::Accent);
                a.fill = a.fill.mix(Color::WHITE, 0.08 * hover_t).mix(Color::BLACK, 0.12 * press_t);
                for s in &mut a.shadows {
                    s.color = s.color.with_alpha(s.color.a * (1.0 - 0.8 * press_t));
                }
                a
            }
            ButtonKind::Ghost => {
                let mut clear = theme.paint(Surface::Hovered);
                clear.fill = Color::TRANSPARENT;
                clear.border = None;
                clear.shadows.clear();
                clear.content = p.text;
                let mut hover = theme.paint(Surface::Inset);
                hover.shadows.clear();
                hover.border = None;
                hover.fill = hover.fill.with_alpha(hover.fill.a * 0.8);
                hover.content = p.text;
                let rest = lerp_paint(&clear, &hover, hover_t);
                lerp_paint(&rest, &theme.paint(Surface::Pressed), press_t)
            }
        };
        if !enabled {
            paint.content = p.faint;
            // The accent's fill is too strong to read faint text on: a
            // button that cannot be pressed is a quiet tint of it, with
            // its label still legible.
            if self.kind == ButtonKind::Accent {
                paint.fill = p.accent.mix(p.surface, 0.72);
                paint.shadows.clear();
                paint.content = p.muted;
            }
        }
        cx.scene.paint(b, radius, &paint);
        cx.focus_ring(b, radius);
        cx.with_content_color(paint.content, |cx| self.content[0].draw(cx));
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        let enabled = self.enabled();
        match event {
            Event::PointerMoved { pos } => {
                let inside = b.contains(*pos);
                let st = cx.state::<ButtonState>();
                if st.hovered != inside {
                    st.hovered = inside;
                    cx.request_redraw();
                }
                if inside && enabled {
                    cx.set_cursor(CursorIcon::Pointer);
                }
                Status::Ignored
            }
            Event::PointerLeft => {
                let st = cx.state::<ButtonState>();
                st.hovered = false;
                st.pressed = false;
                cx.request_redraw();
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) && enabled => {
                cx.state::<ButtonState>().pressed = true;
                cx.request_redraw();
                Status::Captured
            }
            Event::PointerReleased { pos, button: PointerButton::Primary } => {
                let st = cx.state::<ButtonState>();
                if st.pressed {
                    st.pressed = false;
                    cx.request_redraw();
                    if b.contains(*pos)
                        && let Some(m) = self.message(cx.modifiers()) {
                            cx.emit(m);
                        }
                }
                Status::Ignored
            }
            Event::Key(k) if k.pressed && cx.is_focused() && matches!(k.key, Key::Enter | Key::Space) => {
                if let Some(m) = self.message(k.modifiers) {
                    cx.emit(m);
                }
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}
