//! Widgets that display values: progress bars, ring gauges and sparklines.

use neo_render::{FontFamily, Point, Rect, Size, TextLayout, TextStyle};
use neo_theme::{Color, Surface, TextRole};

use crate::core::{Cx, DrawCx, Length, Limits, Widget};

/// A horizontal bar showing progress from 0 to 1.
pub struct ProgressBar {
    value: f32,
    width: Length,
    height: f32,
}

impl ProgressBar {
    pub fn new(value: f32) -> Self {
        Self { value: value.clamp(0.0, 1.0), width: Length::Fill, height: 10.0 }
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

/// Shorthand for [`ProgressBar::new`].
pub fn progress_bar(value: f32) -> ProgressBar {
    ProgressBar::new(value)
}

impl<M> Widget<M> for ProgressBar {
    fn width(&self) -> Length {
        self.width
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        let l = limits.constrain(self.width, Length::Shrink);
        l.resolve(Size::new(if l.max.w.is_finite() { l.max.w } else { 160.0 }, self.height))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let r = b.h * 0.5;
        cx.scene.paint(b, r, &theme.paint(Surface::Inset));
        let inner = b.inset(2.0);
        let w = (inner.w * self.value).max(if self.value > 0.0 { inner.h } else { 0.0 });
        cx.scene.fill(Rect::new(inner.x, inner.y, w, inner.h), inner.h * 0.5, theme.palette().accent, None);
    }
}

/// A circular gauge with a value from 0 to 1 and a label in the middle.
pub struct Gauge {
    value: f32,
    label: String,
    diameter: f32,
    color: Option<Color>,
    layout: Option<TextLayout>,
}

impl Gauge {
    pub fn new(value: f32, label: impl Into<String>) -> Self {
        Self { value: value.clamp(0.0, 1.0), label: label.into(), diameter: 112.0, color: None, layout: None }
    }

    pub fn diameter(mut self, d: f32) -> Self {
        self.diameter = d;
        self
    }

    /// Overrides the accent colour, for example with a warning tone.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
}

/// Shorthand for [`Gauge::new`].
pub fn gauge(value: f32, label: impl Into<String>) -> Gauge {
    Gauge::new(value, label)
}

impl<M> Widget<M> for Gauge {
    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let spec = cx.theme().text(TextRole::Title);
        let style = TextStyle { family: FontFamily::Mono, weight: 500, letter_spacing: 0.0, size: spec.size * self.diameter / 112.0 * 1.05, ..TextStyle::from_spec(spec) };
        self.layout = Some(cx.text().layout(&self.label, &style, None));
        let d = self.diameter.min(limits.max.w).min(limits.max.h);
        limits.resolve(Size::new(d, d))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        let d = b.w.min(b.h);
        let disc = Rect::new(b.x + (b.w - d) * 0.5, b.y + (b.h - d) * 0.5, d, d);
        cx.scene.paint(disc, d * 0.5, &theme.paint(Surface::Raised));
        let ring = disc.inset(d * 0.09);
        let thick = d * 0.08;
        let track = if theme.is_soft() { p.bg.mix(p.shadow_dark, 0.45) } else { p.well.mix(p.line, 0.4) };
        cx.scene.arc(ring, thick, 0.0, std::f32::consts::TAU, track);
        if self.value > 0.0 {
            cx.scene.arc(ring, thick, 0.0, std::f32::consts::TAU * self.value, self.color.unwrap_or(p.accent));
        }
        let core = disc.inset(d * 0.21);
        let mut inset = theme.paint(Surface::Inset);
        if !theme.is_soft() {
            inset.border = None;
            inset.fill = p.surface;
        }
        cx.scene.paint(core, core.w * 0.5, &inset);
        if let Some(l) = &self.layout {
            let s = l.size();
            let c = disc.center();
            cx.scene.text(l, Point::new((c.x - s.w * 0.5).round(), (c.y - s.h * 0.5).round()), p.text);
        }
    }
}

/// A small line chart of recent values.
pub struct Sparkline {
    values: Vec<f32>,
    min: f32,
    max: f32,
    width: Length,
    height: f32,
}

impl Sparkline {
    /// Plots `values` against the range `min..max`.
    pub fn new(values: impl Into<Vec<f32>>, min: f32, max: f32) -> Self {
        Self { values: values.into(), min, max, width: Length::Fill, height: 84.0 }
    }

    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
}

/// Shorthand for [`Sparkline::new`].
pub fn sparkline(values: impl Into<Vec<f32>>, min: f32, max: f32) -> Sparkline {
    Sparkline::new(values, min, max)
}

impl<M> Widget<M> for Sparkline {
    fn width(&self) -> Length {
        self.width
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        let l = limits.constrain(self.width, Length::Shrink);
        l.resolve(Size::new(if l.max.w.is_finite() { l.max.w } else { 240.0 }, self.height))
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let theme = *cx.theme();
        let p = theme.palette();
        for i in 1..4 {
            let y = (b.y + b.h * i as f32 / 4.0).round();
            cx.scene.fill(Rect::new(b.x, y, b.w, 1.0), 0.0, p.line.with_alpha(0.45), None);
        }
        if self.values.len() < 2 {
            return;
        }
        let span = (self.max - self.min).max(f32::EPSILON);
        let n = self.values.len() - 1;
        let pts: Vec<Point> = self
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = ((v - self.min) / span).clamp(0.0, 1.0);
                Point::new(b.x + b.w * i as f32 / n as f32, b.y + 3.0 + (b.h - 6.0) * (1.0 - t))
            })
            .collect();
        cx.scene.area(&pts, b.bottom(), p.accent.with_alpha(0.26), p.accent.with_alpha(0.02));
        cx.scene.polyline(&pts, 2.0, p.accent);
        let last = *pts.last().unwrap();
        cx.scene.fill(Rect::new(last.x - 4.0, last.y - 4.0, 8.0, 8.0), 4.0, p.accent, Some((2.0, p.surface)));
    }
}
