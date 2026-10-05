//! Typed access to the Neo theme from the framework's widget contexts.

use std::sync::OnceLock;

use armature::{Cx, DrawCx};
use armature_render::{Color, Rect};
use neo_theme::Theme;

/// Reads the Neo [`Theme`] inside a widget.
pub trait ThemeCx {
    /// The app's theme. Under an app that is not a Neo [`App`](crate::App),
    /// and so has no theme, this is the default one.
    fn theme(&self) -> &Theme;
}

impl ThemeCx for Cx<'_, '_> {
    fn theme(&self) -> &Theme {
        static DEFAULT: OnceLock<Theme> = OnceLock::new();
        self.style::<Theme>().unwrap_or_else(|| DEFAULT.get_or_init(Theme::default))
    }
}

/// Draws Neo's keyboard focus ring.
pub trait FocusRing {
    /// Draws a focus ring around `rect` when keyboard focus is on this widget.
    fn focus_ring(&mut self, rect: Rect, radius: f32);
}

impl FocusRing for DrawCx<'_, '_> {
    fn focus_ring(&mut self, rect: Rect, radius: f32) {
        if self.focus_visible() {
            let accent = self.theme().palette().accent_text;
            let ring = rect.inset(-3.0);
            self.scene.fill(ring, radius + 3.0, Color::TRANSPARENT, Some((2.0, accent)));
        }
    }
}
