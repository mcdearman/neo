use neo_render::{Point, Size};
use neo_theme::{Color, Surface};

use crate::ThemeCx;
use crate::core::{Align, Cx, DrawCx, Element, EventCx, Length, Limits, Padding, Widget};
use crate::event::{Event, Status};

/// What a container paints behind its content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Background {
    #[default]
    None,
    /// A themed surface: card, well, raised and so on.
    Surface(Surface),
    Color(Color),
    /// Frosted glass: blurs what is behind the container, then tints it.
    Glass,
}

/// Holds one child with padding, sizing, alignment and a background.
pub struct Container<M> {
    child: [Element<M>; 1],
    padding: Padding,
    width: Length,
    height: Length,
    max_width: f32,
    align_x: Align,
    align_y: Align,
    background: Background,
    radius: Option<f32>,
}

impl<M: 'static> Container<M> {
    pub fn new(child: impl Into<Element<M>>) -> Self {
        Self {
            child: [child.into()],
            padding: Padding::ZERO,
            width: Length::Shrink,
            height: Length::Shrink,
            max_width: f32::INFINITY,
            align_x: Align::Start,
            align_y: Align::Start,
            background: Background::None,
            radius: None,
        }
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

    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }

    pub fn align_x(mut self, a: Align) -> Self {
        self.align_x = a;
        self
    }

    pub fn align_y(mut self, a: Align) -> Self {
        self.align_y = a;
        self
    }

    /// Centres the child both ways.
    pub fn center(self) -> Self {
        self.align_x(Align::Center).align_y(Align::Center)
    }

    pub fn background(mut self, b: Background) -> Self {
        self.background = b;
        self
    }

    /// Shorthand for `background(Background::Surface(s))`.
    pub fn surface(self, s: Surface) -> Self {
        self.background(Background::Surface(s))
    }

    /// Corner radius. Defaults to the theme's radius for the surface.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = Some(r);
        self
    }
}

/// Shorthand for [`Container::new`].
pub fn container<M: 'static>(child: impl Into<Element<M>>) -> Container<M> {
    Container::new(child)
}

impl<M: 'static> Widget<M> for Container<M> {
    fn width(&self) -> Length {
        self.width
    }

    fn height(&self) -> Length {
        self.height
    }

    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.child
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let limits = limits.with_max_width(self.max_width).constrain(self.width, self.height);
        let inner = limits.shrink(self.padding);
        let child = &mut self.child[0];
        let mut child_limits = Limits::loose(inner.max);
        if self.align_x == Align::Stretch && inner.max.w.is_finite() {
            child_limits.min.w = inner.max.w;
        }
        if self.align_y == Align::Stretch && inner.max.h.is_finite() {
            child_limits.min.h = inner.max.h;
        }
        let cs = child.layout(cx, child_limits);
        let size = limits.resolve(Size::new(cs.w + self.padding.horizontal(), cs.h + self.padding.vertical()));
        let free_w = size.w - self.padding.horizontal() - cs.w;
        let free_h = size.h - self.padding.vertical() - cs.h;
        child.set_position(Point::new(
            self.padding.left + self.align_x.offset(free_w),
            self.padding.top + self.align_y.offset(free_h),
        ));
        size
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        match self.background {
            Background::None => self.child[0].draw(cx),
            Background::Color(c) => {
                cx.scene.fill(b, self.radius.unwrap_or(0.0), c, None);
                self.child[0].draw(cx);
            }
            Background::Surface(s) => {
                let r = self.radius.unwrap_or(match s {
                    Surface::Window => theme.window_radius(),
                    _ => theme.control_radius(),
                });
                let paint = theme.paint(s);
                cx.scene.paint(b, r, &paint);
                let content = paint.content;
                cx.with_content_color(content, |cx| self.child[0].draw(cx));
            }
            Background::Glass => {
                let r = self.radius.unwrap_or(theme.control_radius());
                let p = theme.palette();
                cx.scene.backdrop(b, r, theme.glass.blur, p.bg.with_alpha(0.45));
                cx.scene.fill(b, r, Color::TRANSPARENT, Some((1.0, p.surface.with_alpha(0.45))));
                self.child[0].draw(cx);
                // Keep later siblings above the glass content.
                cx.scene.push_layer();
            }
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        self.child[0].event(cx, event)
    }
}
